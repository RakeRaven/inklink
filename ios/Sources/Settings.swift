import Combine
import Foundation

enum PencilAction: Codable, Hashable {
    case eraser
    case toolbar
    case shortcut(String)
    case none
}

struct ToolbarButton: Codable, Identifiable, Hashable {
    var id = UUID()
    var label: String
    var combo: String
    /// Held while the finger is down (modifiers), otherwise tapped once.
    var hold: Bool
}

struct Settings: Codable, Equatable {
    var monitorID: UInt32 = 0
    var fps = 60
    var bitrateMbps = 30.0
    var pressureGain = 2.0
    var fingersToHost = true
    var palmRejection = true
    var doubleTap: PencilAction = .eraser
    var squeeze: PencilAction = .toolbar
    var twoFingerTap = "ctrl+z"
    var threeFingerTap = "ctrl+shift+z"
    var showToolbar = true
    var toolbarOnLeft = true
    var buttons: [ToolbarButton] = [
        ToolbarButton(label: "Ctrl", combo: "ctrl", hold: true),
        ToolbarButton(label: "Shift", combo: "shift", hold: true),
        ToolbarButton(label: "Alt", combo: "alt", hold: true),
        ToolbarButton(label: "Space", combo: "space", hold: true),
        ToolbarButton(label: "Undo", combo: "ctrl+z", hold: false),
        ToolbarButton(label: "Redo", combo: "ctrl+shift+z", hold: false),
        ToolbarButton(label: "Brush", combo: "b", hold: false),
        ToolbarButton(label: "[", combo: "[", hold: false),
        ToolbarButton(label: "]", combo: "]", hold: false),
    ]

    private static let key = "settings.v1"

    static func load() -> Settings {
        guard let data = UserDefaults.standard.data(forKey: key),
              let s = try? JSONDecoder().decode(Settings.self, from: data) else { return Settings() }
        return s
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { UserDefaults.standard.set(data, forKey: Self.key) }
    }
}

/// Shared state between the stream screen and the settings sheet.
final class AppModel: ObservableObject {
    @Published var settings = Settings.load() {
        didSet { if settings != oldValue { settings.save() } }
    }
    @Published var host: HostInfo?
    @Published var status = "Waiting for your computer"
    @Published var eraser = false
}
