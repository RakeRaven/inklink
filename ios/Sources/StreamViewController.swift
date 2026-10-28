import Combine
import SwiftUI
import UIKit

final class StreamViewController: UIViewController {
    private let model = AppModel()
    private let link = HostLink()
    private let streamView = StreamView()
    private let toolbar = Toolbar()
    private let statusLabel = UILabel()
    private let toast = UILabel()
    private lazy var renderer = VideoRenderer(layer: streamView.displayLayer)
    private var bag: Set<AnyCancellable> = []
    private var toolbarSide: [NSLayoutConstraint] = []
    private var configuredSize = CGSize.zero
    private var connected = false

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { .all }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        for v in [streamView, statusLabel, toolbar, toast] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        statusLabel.numberOfLines = 0
        statusLabel.textAlignment = .center
        statusLabel.textColor = .lightGray
        statusLabel.font = .systemFont(ofSize: 17)
        toast.textColor = .white
        toast.font = .systemFont(ofSize: 15, weight: .semibold)
        toast.backgroundColor = UIColor.black.withAlphaComponent(0.7)
        toast.layer.cornerRadius = 10
        toast.clipsToBounds = true
        toast.textAlignment = .center
        toast.alpha = 0
        NSLayoutConstraint.activate([
            streamView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            streamView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            streamView.topAnchor.constraint(equalTo: view.topAnchor),
            streamView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            statusLabel.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            statusLabel.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            statusLabel.widthAnchor.constraint(lessThanOrEqualTo: view.widthAnchor, constant: -80),
            toolbar.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            toast.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 16),
            toast.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            toast.widthAnchor.constraint(equalToConstant: 160),
            toast.heightAnchor.constraint(equalToConstant: 36),
        ])

        streamView.send = { [link] in link.send($0) }
        streamView.onPencilAction = { [weak self] in self?.perform($0) }
        streamView.fingers.tap = { [weak self] count in
            guard let self else { return }
            let combo = count == 2 ? model.settings.twoFingerTap : count == 3 ? model.settings.threeFingerTap : ""
            if !combo.isEmpty { link.send(Message.key(.tap, combo)) }
        }
        streamView.fingers.pan = { [weak self] dx, dy in
            guard let self else { return }
            let r = streamView.contentRect
            guard r.width > 0, r.height > 0 else { return }
            link.send(Message.pan(dx: Float(dx / r.width), dy: Float(dy / r.height)))
        }
        toolbar.onKey = { [link] action, combo in link.send(Message.key(action, combo)) }
        toolbar.onSettings = { [weak self] in self?.showSettings() }
        toolbar.onEraser = { [weak self] in self?.perform(.eraser) }

        renderer.onNeedKeyframe = { [link] in link.send(Wire.frame(Wire.keyframe)) }
        renderer.onSize = { [weak self] size in self?.streamView.videoSize = size }
        link.onVideo = { [renderer] in renderer.enqueue($0) }
        link.onHello = { [weak self] info in
            guard let self else { return }
            model.host = info
            if !info.monitors.contains(where: { $0.id == self.model.settings.monitorID }), let first = info.monitors.first {
                model.settings.monitorID = first.id
            }
            sendConfig()
        }
        link.onState = { [weak self] state in self?.apply(state) }

        model.$settings.sink { [weak self] s in self?.applySettings(s) }.store(in: &bag)
        model.$settings
            .map { [$0.monitorID, UInt32($0.fps), UInt32($0.bitrateMbps), UInt32($0.displayMode.wire)] }
            .removeDuplicates()
            .dropFirst()
            .debounce(for: .milliseconds(300), scheduler: RunLoop.main)
            .sink { [weak self] _ in self?.sendConfig() }
            .store(in: &bag)

        // Re-request the stream if video stalls, e.g. after the host's capture restarted.
        Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self, connected, model.host != nil, renderer.secondsSinceFrame > 3 else { return }
            sendConfig()
        }

        apply(.waiting)
        link.start()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        if streamView.bounds.size != configuredSize, model.host != nil { sendConfig() }
    }

    private func sendConfig() {
        guard model.host != nil else { return }
        configuredSize = streamView.bounds.size
        let scale = view.window?.screen.scale ?? traitCollection.displayScale
        let s = model.settings
        link.send(Message.config(
            monitor: s.monitorID, fps: s.fps, kbps: Int(s.bitrateMbps * 1000),
            width: Int(configuredSize.width * scale), height: Int(configuredSize.height * scale),
            mode: s.displayMode))
    }

    private func apply(_ state: HostLink.State) {
        switch state {
        case .waiting:
            connected = false
            model.host = nil
            model.status = "Waiting"
            renderer.reset()
            let ip = HostLink.wifiAddress().map { "Wi-Fi: inklink-host --host \($0)" } ?? "Wi-Fi: not connected"
            statusLabel.text = "Waiting for your computer\n\nUSB: plug in and run inklink-host\n\(ip)"
            statusLabel.isHidden = false
            UIApplication.shared.isIdleTimerDisabled = false
        case .connected(let via):
            connected = true
            model.status = "Connected over \(via)"
            statusLabel.isHidden = true
            UIApplication.shared.isIdleTimerDisabled = true
        }
    }

    private func applySettings(_ s: Settings) {
        streamView.settings = s
        toolbar.reload(s.buttons)
        toolbar.isHidden = !s.showToolbar
        NSLayoutConstraint.deactivate(toolbarSide)
        let guide = view.safeAreaLayoutGuide
        toolbarSide = [s.toolbarOnLeft
            ? toolbar.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 8)
            : toolbar.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -8)]
        NSLayoutConstraint.activate(toolbarSide)
    }

    private func perform(_ action: PencilAction) {
        switch action {
        case .eraser:
            model.eraser.toggle()
            streamView.eraser = model.eraser
            toolbar.setEraser(model.eraser)
            flash(model.eraser ? "Eraser" : "Pen")
        case .toolbar:
            model.settings.showToolbar.toggle()
        case .shortcut(let combo):
            if !combo.isEmpty { link.send(Message.key(.tap, combo)) }
        case .none:
            break
        }
    }

    private func flash(_ text: String) {
        toast.text = text
        toast.alpha = 1
        UIView.animate(withDuration: 0.3, delay: 0.8) { self.toast.alpha = 0 }
    }

    private func showSettings() {
        let sheet = UIHostingController(rootView: SettingsView(model: model))
        sheet.modalPresentationStyle = .formSheet
        present(sheet, animated: true)
    }
}
