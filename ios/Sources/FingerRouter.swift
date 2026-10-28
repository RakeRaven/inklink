import UIKit

/// Holds finger touches back briefly so quick multi-finger taps become shortcuts instead of host touches.
final class FingerRouter {
    var forward: ([TouchSample]) -> Void = { _ in }
    var tap: (Int) -> Void = { _ in }
    var passThrough = true

    private static let window: TimeInterval = 0.25
    private static let slop: CGFloat = 12

    private var ids: [ObjectIdentifier: UInt32] = [:]
    private var nextID: UInt32 = 1
    private var ignored: Set<ObjectIdentifier> = []
    private var deciding = false
    private var pending: [TouchSample] = []
    private var starts: [ObjectIdentifier: CGPoint] = [:]
    private var maxFingers = 0
    private var startTime = Date()
    private var timer: DispatchWorkItem?

    func began(_ touches: [UITouch], in view: UIView, reject: Bool, normalize: (CGPoint) -> (Float, Float)) {
        for t in touches {
            let key = ObjectIdentifier(t)
            if reject {
                ignored.insert(key)
                continue
            }
            if ids.isEmpty {
                deciding = true
                pending = []
                starts = [:]
                maxFingers = 0
                startTime = Date()
                let work = DispatchWorkItem { [weak self] in self?.commit() }
                timer = work
                DispatchQueue.main.asyncAfter(deadline: .now() + Self.window, execute: work)
            }
            ids[key] = nextID
            nextID &+= 1
            let p = t.location(in: view)
            if deciding {
                starts[key] = p
                maxFingers = max(maxFingers, ids.count)
            }
            emit(TouchSample(id: ids[key]!, phase: .down, x: normalize(p).0, y: normalize(p).1))
        }
    }

    func moved(_ touches: [UITouch], in view: UIView, normalize: (CGPoint) -> (Float, Float)) {
        var samples: [TouchSample] = []
        for t in touches {
            guard let id = ids[ObjectIdentifier(t)] else { continue }
            let p = t.location(in: view)
            if deciding, let s = starts[ObjectIdentifier(t)], hypot(p.x - s.x, p.y - s.y) > Self.slop {
                commit()
            }
            let n = normalize(p)
            samples.append(TouchSample(id: id, phase: .move, x: n.0, y: n.1))
        }
        samples.forEach(emit)
    }

    func ended(_ touches: [UITouch], in view: UIView, normalize: (CGPoint) -> (Float, Float)) {
        for t in touches {
            let key = ObjectIdentifier(t)
            if ignored.remove(key) != nil { continue }
            guard let id = ids.removeValue(forKey: key) else { continue }
            let n = normalize(t.location(in: view))
            emit(TouchSample(id: id, phase: .up, x: n.0, y: n.1))
        }
        guard ids.isEmpty, deciding else { return }
        timer?.cancel()
        deciding = false
        if maxFingers >= 2 {
            pending = []
            tap(maxFingers)
        } else {
            flush()
        }
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
