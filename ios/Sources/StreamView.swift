import AVFoundation
import UIKit

/// Shows the computer's screen and turns Pencil, hover and finger input into host events.
final class StreamView: UIView, UIPencilInteractionDelegate {
    override class var layerClass: AnyClass { AVSampleBufferDisplayLayer.self }
    var displayLayer: AVSampleBufferDisplayLayer { layer as! AVSampleBufferDisplayLayer }

    var send: (Data) -> Void = { _ in }
    var onPencilAction: (PencilAction) -> Void = { _ in }
    var settings = Settings()
    var eraser = false
    var videoSize = CGSize(width: 16, height: 9)

    let fingers = FingerRouter()
    private var pen: UITouch?
    private var hovering = false
    private var lastPencil = Date.distantPast
    private var leaveWork: DispatchWorkItem?

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .black
        isMultipleTouchEnabled = true
        let hover = UIHoverGestureRecognizer(target: self, action: #selector(hover(_:)))
        hover.cancelsTouchesInView = false
        addGestureRecognizer(hover)
        let pencil = UIPencilInteraction()
        pencil.delegate = self
        addInteraction(pencil)
        fingers.forward = { [weak self] samples in
            for chunk in stride(from: 0, to: samples.count, by: 32) {
                self?.send(Message.touch(samples[chunk..<min(chunk + 32, samples.count)]))
            }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Where the video actually sits inside the letterboxed layer.
    var contentRect: CGRect { AVMakeRect(aspectRatio: videoSize, insideRect: bounds) }

    private func normalize(_ p: CGPoint) -> (Float, Float) {
        let r = contentRect
        guard r.width > 0, r.height > 0 else { return (0, 0) }
        return (Float(min(max((p.x - r.minX) / r.width, 0), 1)), Float(min(max((p.y - r.minY) / r.height, 0), 1)))
    }

    /// Tilt degrees from altitude/azimuth; positive x leans right, positive y leans toward the user.
    private func tilt(altitude: CGFloat, azimuth: CGFloat) -> (Float, Float) {
        let lean = 1 / tan(max(altitude, 0.01))
        let deg = { (v: CGFloat) in Float(atan(v) * 180 / .pi) }
        return (deg(cos(azimuth) * lean), deg(sin(azimuth) * lean))
    }

    private func sample(_ t: UITouch, _ phase: PenPhase) -> PenSample {
        let (x, y) = normalize(t.preciseLocation(in: self))
        let (tx, ty) = tilt(altitude: t.altitudeAngle, azimuth: t.azimuthAngle(in: self))
        var pressure: Float = 0
        if phase == .down || phase == .move {
            let raw = t.maximumPossibleForce > 0 ? t.force / t.maximumPossibleForce : 0.5
            pressure = Float(min(raw * settings.pressureGain, 1))
        }
        var twist: Float = 0
        if #available(iOS 17.5, *) { twist = Float(t.rollAngle * 180 / .pi) }
        return PenSample(phase: phase, eraser: eraser, x: x, y: y, pressure: pressure, tiltX: tx, tiltY: ty, twist: twist)
    }

    // Hands stay planted between strokes, so fingers are ignored for a moment after the pen.
    private var pencilBusy: Bool {
        pen != nil || hovering || Date().timeIntervalSince(lastPencil) < 1.0
    }

    /// Fingertips report radii of about 20-42 pt (in steps); a resting palm or hand edge is larger.
    private static func isPalm(_ t: UITouch) -> Bool {
        t.majorRadius > 50
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        var direct: [UITouch] = []
        for t in touches {
            if t.type == .pencil {
                guard pen == nil else { continue }
                pen = t
                leaveWork?.cancel()
                if settings.palmRejection { fingers.cancelAll() }
                send(Message.pen([sample(t, .down)]))
            } else if t.type == .direct {
                direct.append(t)
            }
        }
        if !direct.isEmpty {
            fingers.passThrough = settings.fingersToHost
            fingers.oneFingerDrags = settings.oneFingerDrags
            let reject = settings.palmRejection
            let busy = pencilBusy
            fingers.began(direct, in: self, reject: { reject && (busy || Self.isPalm($0)) }, normalize: normalize)
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let pen, touches.contains(pen) {
            // Coalesced touches carry the full 240 Hz Pencil stream.
            let all = event?.coalescedTouches(for: pen) ?? [pen]
            send(Message.pen(all.map { sample($0, .move) }))
            lastPencil = Date()
        }
        let direct = touches.filter { $0.type == .direct }
        if !direct.isEmpty {
            let reject = settings.palmRejection
            fingers.moved(Array(direct), in: self, isPalm: { reject && Self.isPalm($0) }, normalize: normalize)
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        finish(touches)
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        finish(touches)
    }

    private func finish(_ touches: Set<UITouch>) {
        if let pen, touches.contains(pen) {
            send(Message.pen([sample(pen, .up)]))
            self.pen = nil
            lastPencil = Date()
            // iPads without hover never report leaving, so leave unless hover picks the pen up.
            let work = DispatchWorkItem { [weak self] in
                guard let self, self.pen == nil, !self.hovering else { return }
                self.send(Message.pen([PenSample(phase: .leave, eraser: self.eraser, x: 0, y: 0, pressure: 0, tiltX: 0, tiltY: 0, twist: 0)]))
            }
            leaveWork = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
        }
        let direct = touches.filter { $0.type == .direct }
        if !direct.isEmpty { fingers.ended(Array(direct), in: self, normalize: normalize) }
    }

    @objc private func hover(_ g: UIHoverGestureRecognizer) {
        guard pen == nil else { return }
        switch g.state {
        case .began, .changed:
            hovering = true
            leaveWork?.cancel()
            let (x, y) = normalize(g.location(in: self))
            let (tx, ty) = tilt(altitude: g.altitudeAngle, azimuth: g.azimuthAngle(in: self))
            var twist: Float = 0
            if #available(iOS 17.5, *) { twist = Float(g.rollAngle * 180 / .pi) }
            send(Message.pen([PenSample(phase: .hover, eraser: eraser, x: x, y: y, pressure: 0, tiltX: tx, tiltY: ty, twist: twist)]))
        default:
            hovering = false
            lastPencil = Date()
            send(Message.pen([PenSample(phase: .leave, eraser: eraser, x: 0, y: 0, pressure: 0, tiltX: 0, tiltY: 0, twist: 0)]))
        }
    }

    // Pencil 2 / Pro double-tap and Pencil Pro squeeze.
    func pencilInteractionDidTap(_ interaction: UIPencilInteraction) {
        onPencilAction(settings.doubleTap)
    }

    @available(iOS 17.5, *)
    func pencilInteraction(_ interaction: UIPencilInteraction, didReceiveTap tap: UIPencilInteraction.Tap) {
        onPencilAction(settings.doubleTap)
    }

    @available(iOS 17.5, *)
    func pencilInteraction(_ interaction: UIPencilInteraction, didReceiveSqueeze squeeze: UIPencilInteraction.Squeeze) {
        if squeeze.phase == .ended { onPencilAction(settings.squeeze) }
    }
}
