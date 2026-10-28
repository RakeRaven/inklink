import UIKit

/// Floating strip of shortcut buttons along one edge.
final class Toolbar: UIView {
    var onKey: (KeyAction, String) -> Void = { _, _ in }
    var onSettings: () -> Void = {}
    var onEraser: () -> Void = {}

    private let stack = UIStackView()
    private let eraserButton = UIButton(type: .system)
    private var actions: [UIButton: ToolbarButton] = [:]

    override init(frame: CGRect) {
        super.init(frame: frame)
        let blur = UIVisualEffectView(effect: UIBlurEffect(style: .systemThinMaterialDark))
        blur.layer.cornerRadius = 14
        blur.clipsToBounds = true
        blur.translatesAutoresizingMaskIntoConstraints = false
        addSubview(blur)
        stack.axis = .vertical
        stack.spacing = 4
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            blur.leadingAnchor.constraint(equalTo: leadingAnchor),
            blur.trailingAnchor.constraint(equalTo: trailingAnchor),
            blur.topAnchor.constraint(equalTo: topAnchor),
            blur.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 6),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -6),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func reload(_ buttons: [ToolbarButton]) {
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        actions = [:]

        let gear = iconButton("gearshape")
        gear.addAction(UIAction { [weak self] _ in self?.onSettings() }, for: .touchUpInside)
        stack.addArrangedSubview(gear)

        eraserButton.setImage(UIImage(systemName: "eraser"), for: .normal)
        style(eraserButton)
        eraserButton.removeTarget(nil, action: nil, for: .allEvents)
        eraserButton.addAction(UIAction { [weak self] _ in self?.onEraser() }, for: .touchUpInside)
        stack.addArrangedSubview(eraserButton)

        for b in buttons where !b.combo.isEmpty {
            let button = UIButton(type: .system)
            button.setTitle(b.label, for: .normal)
            button.titleLabel?.font = .systemFont(ofSize: 15, weight: .semibold)
            button.titleLabel?.adjustsFontSizeToFitWidth = true
            style(button)
            actions[button] = b
            if b.hold {
                button.addTarget(self, action: #selector(press(_:)), for: .touchDown)
                button.addTarget(self, action: #selector(release(_:)), for: [.touchUpInside, .touchUpOutside, .touchCancel])
            } else {
                button.addTarget(self, action: #selector(tap(_:)), for: .touchUpInside)
            }
            stack.addArrangedSubview(button)
        }
    }

    func setEraser(_ on: Bool) {
        eraserButton.backgroundColor = on ? .systemPink.withAlphaComponent(0.6) : UIColor.white.withAlphaComponent(0.08)
    }

    private func iconButton(_ symbol: String) -> UIButton {
        let b = UIButton(type: .system)
        b.setImage(UIImage(systemName: symbol), for: .normal)
        style(b)
        return b
    }

    private func style(_ b: UIButton) {
        b.tintColor = .white
        b.setTitleColor(.white, for: .normal)
        b.backgroundColor = UIColor.white.withAlphaComponent(0.08)
        b.layer.cornerRadius = 10
        b.translatesAutoresizingMaskIntoConstraints = false
        b.widthAnchor.constraint(equalToConstant: 56).isActive = true
        b.heightAnchor.constraint(equalToConstant: 44).isActive = true
    }

    @objc private func press(_ b: UIButton) {
        guard let a = actions[b] else { return }
        b.backgroundColor = .systemPink.withAlphaComponent(0.6)
        onKey(.down, a.combo)
    }

    @objc private func release(_ b: UIButton) {
        guard let a = actions[b] else { return }
        b.backgroundColor = UIColor.white.withAlphaComponent(0.08)
        onKey(.up, a.combo)
    }

    @objc private func tap(_ b: UIButton) {
        guard let a = actions[b] else { return }
        onKey(.tap, a.combo)
    }
}
