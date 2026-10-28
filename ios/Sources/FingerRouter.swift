import UIKit

/// Decides what fingers mean: quick multi-finger taps become shortcuts, two or more fingers go to the
/// host as touch (pinch, pan, rotate), one-finger drags are held back so they can't draw, and palms are dropped.
final class FingerRouter {
    var forward: ([TouchSample]) -> Void = { _ in }
    var tap: (Int) -> Void = { _ in }
    var passThrough = true
    /// Send one-finger drags to the host too (most drawing apps would draw with them).
    var oneFingerDrags = false
    /// Three-finger drags move the Fill slice instead of reaching the host; deltas are in points.
    var panEnabled = false
    var pan: (CGFloat, CGFloat) -> Void = { _, _ in }

    private enum State { case idle, deciding, holding, live, panning }

    private static let window: TimeInterval = 0.25
    private static let slop: CGFloat = 12

    private var state = State.idle
    private var ids: [ObjectIdentifier: UInt32] = [:]
    private var last: [UInt32: (Float, Float)] = [:]
    private var sent: Set<UInt32> = []
    private var nextID: UInt32 = 1
    private var ignored: Set<ObjectIdentifier> = []
    private var dropped = false
    private var pending: [TouchSample] = []
    private var starts: [ObjectIdentifier: CGPoint] = [:]
    private var maxFingers = 0
    private var timer: DispatchWorkItem?
    private var points: [ObjectIdentifier: CGPoint] = [:]
    private var centroid: CGPoint?

    func began(_ touches: [UITouch], in view: UIView, reject: (UITouch) -> Bool, normalize: (CGPoint) -> (Float, Float)) {
        for t in touches {
            let key = ObjectIdentifier(t)
            if reject(t) {
                ignored.insert(key)
                continue
            }
            if state == .idle {
                state = .deciding
                dropped = false
                pending = []
                starts = [:]
                maxFingers = 0
                let work = DispatchWorkItem { [weak self] in self?.decide() }
                timer = work
                DispatchQueue.main.asyncAfter(deadline: .now() + Self.window, execute: work)
            }
            let id = nextID
            nextID &+= 1
            ids[key] = id
            let p = t.location(in: view)
            let n = normalize(p)
            last[id] = n
            points[key] = p
            centroid = nil
            maxFingers = max(maxFingers, ids.count)
            switch state {
            case .deciding:
                starts[key] = p
                pending.append(TouchSample(id: id, phase: .down, x: n.0, y: n.1))
            case .holding where ids.count >= 2:
                goLive()
            case .live:
                send([TouchSample(id: id, phase: .down, x: n.0, y: n.1)])
            default:
                break
            }
        }
    }

    /// `isPalm` lets a touch that grows into a resting hand be dropped mid-gesture.
    func moved(_ touches: [UITouch], in view: UIView, isPalm: (UITouch) -> Bool, normalize: (CGPoint) -> (Float, Float)) {
        var samples: [TouchSample] = []
        for t in touches {
            let key = ObjectIdentifier(t)
            guard let id = ids[key] else { continue }
            if isPalm(t) {
                drop(key)
                continue
            }
            let p = t.location(in: view)
            let n = normalize(p)
            last[id] = n
            points[key] = p
            if state == .deciding, let s = starts[key], hypot(p.x - s.x, p.y - s.y) > Self.slop {
                decide()
            }
            switch state {
            case .deciding: pending.append(TouchSample(id: id, phase: .move, x: n.0, y: n.1))
            case .live: samples.append(TouchSample(id: id, phase: .move, x: n.0, y: n.1))
            default: break
            }
        }
        if state == .panning, !points.isEmpty {
            let c = CGPoint(x: points.values.map(\.x).reduce(0, +) / CGFloat(points.count),
                            y: points.values.map(\.y).reduce(0, +) / CGFloat(points.count))
            if let old = centroid { pan(c.x - old.x, c.y - old.y) }
            centroid = c
        }
        send(samples)
    }

    func ended(_ touches: [UITouch], in view: UIView, normalize: (CGPoint) -> (Float, Float)) {
        var ups: [TouchSample] = []
        for t in touches {
            let key = ObjectIdentifier(t)
            if ignored.remove(key) != nil { continue }
            guard let id = ids.removeValue(forKey: key) else { continue }
            last[id] = nil
            points[key] = nil
            centroid = nil
            let n = normalize(t.location(in: view))
            let up = TouchSample(id: id, phase: .up, x: n.0, y: n.1)
            if state == .deciding { pending.append(up) } else { ups.append(up) }
        }
        send(ups)
        guard ids.isEmpty else { return }
        if state == .deciding {
            timer?.cancel()
            if maxFingers >= 2, !dropped {
                tap(maxFingers)
            } else if !dropped {
                // A quick one-finger tap is a click.
                send(pending)
            }
        }
        reset()
    }

    /// The Pencil came down: every finger on the glass is now treated as a palm.
    func cancelAll() {
        for key in Array(ids.keys) { drop(key) }
    }

    /// End of the tap window or the first real movement: pick between touch, holding back, or waiting.
    private func decide() {
        guard state == .deciding else { return }
        timer?.cancel()
        if panEnabled, ids.count >= 3 {
            state = .panning
            centroid = nil
        } else if ids.count >= 2 || (oneFingerDrags && !ids.isEmpty) {
            state = .live
            send(pending.filter { s in ids.values.contains(s.id) || s.phase == .up })
        } else {
            state = .holding
        }
        pending = []
    }

    /// A second finger joined a held one-finger touch: start both from where they are now.
    private func goLive() {
        state = .live
        send(ids.values.compactMap { id in last[id].map { TouchSample(id: id, phase: .down, x: $0.0, y: $0.1) } })
    }

    private func drop(_ key: ObjectIdentifier) {
        guard let id = ids.removeValue(forKey: key) else { return }
        ignored.insert(key)
        points[key] = nil
        centroid = nil
        dropped = true
        pending.removeAll { $0.id == id }
        if sent.contains(id), let p = last[id] {
            send([TouchSample(id: id, phase: .up, x: p.0, y: p.1)])
        }
        last[id] = nil
        if ids.isEmpty {
            timer?.cancel()
            reset()
        }
    }

    private func reset() {
        state = .idle
        points = [:]
        centroid = nil
        pending = []
        sent = []
    }

    /// Only forwards moves and lifts for touches whose down the host has seen.
    private func send(_ samples: [TouchSample]) {
        guard passThrough else { return }
        let out = samples.filter { s in
            switch s.phase {
            case .down: sent.insert(s.id); return true
            case .move: return sent.contains(s.id)
            case .up: return sent.remove(s.id) != nil
            }
        }
        if !out.isEmpty { forward(out) }
    }
}
