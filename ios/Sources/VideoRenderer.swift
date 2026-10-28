import AVFoundation
import CoreMedia

/// Turns Annex B H.264 access units into immediately displayed sample buffers.
final class VideoRenderer {
    let layer: AVSampleBufferDisplayLayer
    var onNeedKeyframe: (() -> Void)?
    /// Called on the main queue when the stream resolution changes.
    var onSize: ((CGSize) -> Void)?

    private let lock = NSLock()
    private var format: CMVideoFormatDescription?
    private var sps = Data()
    private var pps = Data()
    private var waitingForIDR = true
    private var lastKeyframeRequest = Date.distantPast
    private var lastFrame = Date.distantPast

    init(layer: AVSampleBufferDisplayLayer) {
        self.layer = layer
        layer.videoGravity = .resizeAspect
    }

    var secondsSinceFrame: TimeInterval {
        lock.lock(); defer { lock.unlock() }
        return Date().timeIntervalSince(lastFrame)
    }

    func reset() {
        lock.lock()
        format = nil
        sps = Data()
        pps = Data()
        waitingForIDR = true
        lock.unlock()
        layer.sampleBufferRenderer.flush(removingDisplayedImage: true) {}
    }

    func enqueue(_ au: Data) {
        lock.lock(); defer { lock.unlock() }
        lastFrame = Date()
        let bytes = [UInt8](au)
        var avcc = Data(capacity: bytes.count + 16)
        var newSPS: Data?, newPPS: Data?
        var isIDR = false
        for nal in Self.nalUnits(bytes) {
            switch nal.first! & 0x1F {
            case 7: newSPS = Data(nal)
            case 8: newPPS = Data(nal)
            case 9: continue
            case let type:
                if type == 5 { isIDR = true }
                withUnsafeBytes(of: UInt32(nal.count).bigEndian) { avcc.append(contentsOf: $0) }
                avcc.append(contentsOf: nal)
            }
        }
        if let s = newSPS, let p = newPPS, s != sps || p != pps {
            sps = s
            pps = p
            format = Self.makeFormat(sps: s, pps: p)
            if let format {
                let d = CMVideoFormatDescriptionGetDimensions(format)
                let size = CGSize(width: Int(d.width), height: Int(d.height))
                DispatchQueue.main.async { self.onSize?(size) }
            }
        }

        let renderer = layer.sampleBufferRenderer
        if renderer.status == .failed {
            renderer.flush()
            waitingForIDR = true
        }
        if isIDR { waitingForIDR = false }
        guard let format, !waitingForIDR, !avcc.isEmpty else {
            requestKeyframe()
            return
        }
        guard let sample = Self.makeSample(avcc, format: format) else { return }
        renderer.enqueue(sample)
    }

    private func requestKeyframe() {
        guard Date().timeIntervalSince(lastKeyframeRequest) > 0.5 else { return }
        lastKeyframeRequest = Date()
        onNeedKeyframe?()
    }

    static func nalUnits(_ b: [UInt8]) -> [ArraySlice<UInt8>] {
        var starts: [(code: Int, data: Int)] = []
        var i = 0
        while i + 2 < b.count {
            if b[i] == 0, b[i + 1] == 0, b[i + 2] == 1 {
                let code = i > 0 && b[i - 1] == 0 ? i - 1 : i
                starts.append((code, i + 3))
                i += 3
            } else {
                i += 1
            }
        }
        return starts.indices.compactMap { k in
            let end = k + 1 < starts.count ? starts[k + 1].code : b.count
            let begin = starts[k].data
            return begin < end ? b[begin..<end] : nil
        }
    }

    private static func makeFormat(sps: Data, pps: Data) -> CMVideoFormatDescription? {
        var format: CMVideoFormatDescription?
        let status = sps.withUnsafeBytes { s in
            pps.withUnsafeBytes { p in
                let pointers = [s.bindMemory(to: UInt8.self).baseAddress!, p.bindMemory(to: UInt8.self).baseAddress!]
                let sizes = [sps.count, pps.count]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: 2, parameterSetPointers: pointers,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, formatDescriptionOut: &format)
            }
        }
        return status == noErr ? format : nil
    }

    private static func makeSample(_ avcc: Data, format: CMVideoFormatDescription) -> CMSampleBuffer? {
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: avcc.count, blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil, offsetToData: 0, dataLength: avcc.count, flags: 0, blockBufferOut: &block) == noErr,
            let block else { return nil }
        let copied = avcc.withUnsafeBytes {
            CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: avcc.count)
        }
        guard copied == noErr else { return nil }

        var sample: CMSampleBuffer?
        var size = avcc.count
        guard CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format, sampleCount: 1,
            sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1, sampleSizeArray: &size,
            sampleBufferOut: &sample) == noErr, let sample else { return nil }

        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dict, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return sample
    }
}
