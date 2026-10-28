import Foundation

// Wire format shared with host/src/protocol.rs: [u8 kind][u32 LE length][payload].
enum Wire {
    static let port: UInt16 = 47800
    static let version: UInt16 = 2
    static let bonjourType = "_inklink._tcp."

    static let hostHello: UInt8 = 0x01
    static let video: UInt8 = 0x02
    static let clientHello: UInt8 = 0x10
    static let streamConfig: UInt8 = 0x11
    static let keyframe: UInt8 = 0x12
    static let pan: UInt8 = 0x13
    static let pen: UInt8 = 0x20
    static let touch: UInt8 = 0x21
    static let key: UInt8 = 0x22

    static func frame(_ kind: UInt8, _ payload: Data = Data()) -> Data {
        var w = ByteWriter()
        w.u8(kind)
        w.u32(UInt32(payload.count))
        w.data.append(payload)
        return w.data
    }
}

struct ByteWriter {
    var data = Data()
    mutating func u8(_ v: UInt8) { data.append(v) }
    mutating func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
    mutating func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
    mutating func f32(_ v: Float) { u32(v.bitPattern) }
    mutating func str(_ s: String) {
        let bytes = Array(s.utf8.prefix(Int(UInt16.max)))
        u16(UInt16(bytes.count))
        data.append(contentsOf: bytes)
    }
}

struct ByteReader {
    struct Short: Error {}
    private let bytes: [UInt8]
    private var pos = 0
    init(_ data: Data) { bytes = [UInt8](data) }

    mutating func take(_ n: Int) throws -> ArraySlice<UInt8> {
        guard pos + n <= bytes.count else { throw Short() }
        defer { pos += n }
        return bytes[pos..<pos + n]
    }
    mutating func u8() throws -> UInt8 { try take(1).first! }
    mutating func u16() throws -> UInt16 { try take(2).reversed().reduce(0) { $0 << 8 | UInt16($1) } }
    mutating func u32() throws -> UInt32 { try take(4).reversed().reduce(0) { $0 << 8 | UInt32($1) } }
    mutating func str() throws -> String { String(decoding: try take(Int(u16())), as: UTF8.self) }
}

struct HostMonitor: Identifiable, Hashable {
    let id: UInt32
    let width: UInt32
    let height: UInt32
    let name: String
}

struct HostInfo {
    let name: String
    let isWindows: Bool
    let monitors: [HostMonitor]

    init(_ payload: Data) throws {
        var r = ByteReader(payload)
        _ = try r.u16()
        isWindows = try r.u8() == 0
        name = try r.str()
        var list: [HostMonitor] = []
        for _ in 0..<(try r.u8()) {
            let id = try r.u32(), w = try r.u32(), h = try r.u32()
            list.append(HostMonitor(id: id, width: w, height: h, name: try r.str()))
        }
        monitors = list
    }
}

enum PenPhase: UInt8 { case hover, down, move, up, leave }

struct PenSample {
    var phase: PenPhase
    var eraser: Bool
    var x: Float, y: Float
    var pressure: Float
    var tiltX: Float, tiltY: Float
    var twist: Float
}

enum TouchPhase: UInt8 { case down, move, up }

struct TouchSample {
    var id: UInt32
    var phase: TouchPhase
    var x: Float, y: Float
}

enum KeyAction: UInt8 { case up, down, tap }

enum Message {
    static func hello() -> Data {
        var w = ByteWriter()
        w.u16(Wire.version)
        return Wire.frame(Wire.clientHello, w.data)
    }

    static func config(monitor: UInt32, fps: Int, kbps: Int, width: Int, height: Int, mode: DisplayMode) -> Data {
        var w = ByteWriter()
        w.u32(monitor)
        w.u16(UInt16(fps))
        w.u32(UInt32(kbps))
        w.u32(UInt32(width))
        w.u32(UInt32(height))
        w.u8(mode.wire)
        return Wire.frame(Wire.streamConfig, w.data)
    }

    /// Moves the Fill slice by a fraction of the visible area.
    static func pan(dx: Float, dy: Float) -> Data {
        var w = ByteWriter()
        w.f32(dx)
        w.f32(dy)
        return Wire.frame(Wire.pan, w.data)
    }

    static func pen(_ samples: [PenSample]) -> Data {
        var w = ByteWriter()
        for s in samples {
            w.u8(s.phase.rawValue)
            w.u8(s.eraser ? 1 : 0)
            for v in [s.x, s.y, s.pressure, s.tiltX, s.tiltY, s.twist] { w.f32(v) }
        }
        return Wire.frame(Wire.pen, w.data)
    }

    static func touch(_ samples: ArraySlice<TouchSample>) -> Data {
        var w = ByteWriter()
        w.u8(UInt8(samples.count))
        for s in samples {
            w.u32(s.id)
            w.u8(s.phase.rawValue)
            w.f32(s.x)
            w.f32(s.y)
        }
        return Wire.frame(Wire.touch, w.data)
    }

    static func key(_ action: KeyAction, _ combo: String) -> Data {
        var w = ByteWriter()
        w.u8(action.rawValue)
        w.str(combo)
        return Wire.frame(Wire.key, w.data)
    }
}
