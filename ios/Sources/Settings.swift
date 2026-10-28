import Combine
import Foundation

enum PencilAction: Codable, Hashable {
    case eraser
    case toolbar
    case shortcut(String)
    case none
}

enum DisplayMode: String, Codable, CaseIterable, Identifiable {
    /// The whole monitor, with bars where the shapes differ.
    case fit
    /// An iPad-shaped slice of the monitor, panned with three fingers.
    case fill
    /// A new extended monitor at the iPad's resolution.
    case virtual

    var id: Self { self }
    var wire: UInt8 { switch self { case .fit: 0; case .fill: 1; case .virtual: 2 } }
    var title: String { switch self { case .fit: "Mirror (fit)"; case .fill: "Mirror (fill)"; case .virtual: "Extra display" } }
}

struct ToolbarButton: Codable, Identifiable, Hashable {
    var id = UUID()
    var label: String
    var combo: String
    /// Held while the finger is down (modifiers), otherwise tapped once.
    var hold: Bool
}

struct Settings: Codable, Equatable {
    var displayMode = DisplayMode.fit
    var monitorID: UInt32 = 0
    var fps = 60
    var bitrateMbps = 30.0
    var pressureGain = 2.0
    var fingersToHost = true
    var oneFingerDrags = false
    var palmRejection = true
    var doubleTap: PencilAction = .eraser
    var squeeze: PencilAction = .toolbar
    var twoFingerTap = "ctrl+z"
    var threeFingerTap = "ctrl+shift+z"
    var flipHorizontal = "ctrl+alt+shift+h"
    var flipVertical = "ctrl+alt+shift+v"
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

    init() {}

    // Missing keys fall back to defaults, so adding a setting never wipes the saved ones.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = Settings()
        func get<T: Decodable>(_ key: CodingKeys, _ fallback: T) -> T {
            (try? c.decodeIfPresent(T.self, forKey: key)) ?? fallback
        }
        displayMode = get(.displayMode, d.displayMode)
        monitorID = get(.monitorID, d.monitorID)
        fps = get(.fps, d.fps)
        bitrateMbps = get(.bitrateMbps, d.bitrateMbps)
        pressureGain = get(.pressureGain, d.pressureGain)
        fingersToHost = get(.fingersToHost, d.fingersToHost)
        oneFingerDrags = get(.oneFingerDrags, d.oneFingerDrags)
        palmRejection = get(.palmRejection, d.palmRejection)
        doubleTap = get(.doubleTap, d.doubleTap)
        squeeze = get(.squeeze, d.squeeze)
        twoFingerTap = get(.twoFingerTap, d.twoFingerTap)
        threeFingerTap = get(.threeFingerTap, d.threeFingerTap)
        flipHorizontal = get(.flipHorizontal, d.flipHorizontal)
        flipVertical = get(.flipVertical, d.flipVertical)
        showToolbar = get(.showToolbar, d.showToolbar)
        toolbarOnLeft = get(.toolbarOnLeft, d.toolbarOnLeft)
        buttons = get(.buttons, d.buttons)
    }

    // A file in Documents rather than UserDefaults, which LiveContainer redirects per guest app.
    private static var url: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("settings.json")
    }

    static func load() -> Settings {
        guard let data = try? Data(contentsOf: url) ?? UserDefaults.standard.data(forKey: "settings.v1"),
              let s = try? JSONDecoder().decode(Settings.self, from: data) else { return Settings() }
        return s
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { try? data.write(to: Self.url, options: .atomic) }
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
