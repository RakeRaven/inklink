import UIKit

/// Holds finger touches back briefly so quick multi-finger taps become shortcuts instead of host touches,
/// and drops touches that turn out to be a palm.
final class FingerRouter {
    var forward: ([TouchSample]) -> Void = { _ in }
    var tap: (Int) -> Void = { _ in }
    var passThrough = true

    private static let window: TimeInterval = 0.25
    private static let slop: CGFloat = 12

    private var ids: [ObjectIdentifier: UInt32] = [:]
    private var last: [UInt32: (Float, Float)] = [:]
    private var nextID: UInt32 = 1
    private var ignored: Set<ObjectIdentifier> = []
    private var deciding = false
    private var dropped = false
    private var pending: [TouchSample] = []
    private var starts: [ObjectIdentifier: CGPoint] = [:]
    private var maxFingers = 0
    private var timer: DispatchWorkItem?

    func began(_ touches: [UITouch], in view: UIView, reject: (UITouch) -> Bool, normalize: (CGPoint) -> (Float, Float)) {
        for t in touches {
            let key = ObjectIdentifier(t)
            if reject(t) {
                ignored.insert(key)
                continue
            }
            if ids.isEmpty {
                deciding = true
                dropped = false
                pending = []
                starts = [:]
                maxFingers = 0
                let work = DispatchWorkItem { [weak self] in self?.commit() }
                timer = work
                DispatchQueue.main.asyncAfter(deadline: .now() + Self.window, execute: work)
            }
            let id = nextID
            nextID &+= 1
            ids[key] = id
            let p = t.location(in: view)
            if deciding {
                starts[key] = p
                maxFingers = max(maxFingers, ids.count)
            }
            let n = normalize(p)
            last[id] = n
            emit(TouchSample(id: id, phase: .down, x: n.0, y: n.1))
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
            if deciding, let s = starts[key], hypot(p.x - s.x, p.y - s.y) > Self.slop {
                commit()
            }
            let n = normalize(p)
            last[id] = n
            samples.append(TouchSample(id: id, phase: .move, x: n.0, y: n.1))
        }
        samples.forEach(emit)
    }

    func ended(_ touches: [UITouch], in view: UIView, normalize: (CGPoint) -> (Float, Float)) {
        for t in touches {
            let key = ObjectIdentifier(t)
            if ignored.remove(key) != nil { continue }
            guard let id = ids.removeValue(forKey: key) else { continue }
            last[id] = nil
            let n = normalize(t.location(in: view))
            emit(TouchSample(id: id, phase: .up, x: n.0, y: n.1))
        }
        guard ids.isEmpty, deciding else { return }
        timer?.cancel()
        deciding = false
        if maxFingers >= 2, !dropped {
            pending = []
            tap(maxFingers)
        } else {
            flush()
        }
    }

    /// The Pencil came down: every finger on the glass is now treated as a palm.
    func cancelAll() {
        for key in Array(ids.keys) { drop(key) }
    }

    private func drop(_ key: ObjectIdentifier) {
        guard let id = ids.removeValue(forKey: key) else { return }
        ignored.insert(key)
        if deciding {
            dropped = true
            pending.removeAll { $0.id == id }
            if ids.isEmpty {
                timer?.cancel()
                deciding = false
                pending = []
            }
        } else if passThrough, let p = last[id] {
            forward([TouchSample(id: id, phase: .up, x: p.0, y: p.1)])
        }
        last[id] = nil
    }

    private func emit(_ s: TouchSample) {
        if deciding {
            pending.append(s)
        } else if passThrough {
            forward([s])
        }
    }

    private func commit() {
        guard deciding else { return }
        timer?.cancel()
        deciding = false
        flush()
    }

    private func flush() {
        if passThrough, !pending.isEmpty { forward(pending) }
        pending = []
    }
}
