import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Computer") {
                    LabeledContent("Status", value: model.status)
                    if let host = model.host {
                        Picker("Screen", selection: $model.settings.monitorID) {
                            ForEach(host.monitors) { m in
                                Text("\(m.name) · \(m.width)×\(m.height)").tag(m.id)
                            }
                        }
                    }
                    Picker("Frame rate", selection: $model.settings.fps) {
                        ForEach([30, 60, 90, 120], id: \.self) { Text("\($0) fps").tag($0) }
                    }
                    VStack(alignment: .leading) {
                        Text("Bitrate: \(Int(model.settings.bitrateMbps)) Mbps")
                        Slider(value: $model.settings.bitrateMbps, in: 5...100, step: 5)
                    }
                }

                Section {
                    VStack(alignment: .leading) {
                        Text("Pressure boost: \(model.settings.pressureGain, specifier: "%.1f")×")
                        Slider(value: $model.settings.pressureGain, in: 1...4, step: 0.1)
                    }
                    PencilActionPicker(title: "Double-tap", action: $model.settings.doubleTap)
                    PencilActionPicker(title: "Squeeze (Pencil Pro)", action: $model.settings.squeeze)
                } header: {
                    Text("Pencil")
                } footer: {
                    Text("Eraser switches the pen to its eraser end, so drawing apps pick their eraser tool.")
                }

                Section {
                    Toggle("Send fingers to computer", isOn: $model.settings.fingersToHost)
                    Toggle("One-finger drags too", isOn: $model.settings.oneFingerDrags)
                    Toggle("Ignore palm while drawing", isOn: $model.settings.palmRejection)
                    ComboField(title: "Two-finger tap", combo: $model.settings.twoFingerTap)
                    ComboField(title: "Three-finger tap", combo: $model.settings.threeFingerTap)
                } header: {
                    Text("Touch")
                } footer: {
                    Text("Two or more fingers arrive as a touchscreen, so pinch, rotate and pan work the way your app does on a touch PC. One-finger taps click; one-finger drags are held back so they can't draw unless you turn them on.")
                }

                Section {
                    Toggle("Show toolbar", isOn: $model.settings.showToolbar)
                    Toggle("Toolbar on the left", isOn: $model.settings.toolbarOnLeft)
                    ForEach($model.settings.buttons) { $button in
                        HStack {
                            TextField("Label", text: $button.label).frame(maxWidth: 110)
                            TextField("Keys", text: $button.combo)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .foregroundStyle(.secondary)
                            Toggle("Hold", isOn: $button.hold).fixedSize()
                        }
                    }
                    .onDelete { model.settings.buttons.remove(atOffsets: $0) }
                    .onMove { model.settings.buttons.move(fromOffsets: $0, toOffset: $1) }
                    Button("Add button") {
                        model.settings.buttons.append(ToolbarButton(label: "New", combo: "", hold: false))
                    }
                } header: {
                    Text("Toolbar")
                } footer: {
                    Text("Keys look like ctrl+shift+z, alt, space, f5, [ or ]. Hold keeps them pressed while your finger is down.")
                }
            }
            .navigationTitle("InkLink")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { EditButton() }
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
        }
    }
}

private struct ComboField: View {
    let title: String
    @Binding var combo: String

    var body: some View {
        LabeledContent(title) {
            TextField("none", text: $combo)
                .multilineTextAlignment(.trailing)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
        }
    }
}

private struct PencilActionPicker: View {
    let title: String
    @Binding var action: PencilAction

    private enum Kind: Hashable { case eraser, toolbar, shortcut, none }

    private var kind: Binding<Kind> {
        Binding {
            switch action {
            case .eraser: .eraser
            case .toolbar: .toolbar
            case .shortcut: .shortcut
            case .none: .none
            }
        } set: { k in
            switch k {
            case .eraser: action = .eraser
            case .toolbar: action = .toolbar
            case .shortcut: if case .shortcut = action {} else { action = .shortcut("e") }
            case .none: action = .none
            }
        }
    }

    private var combo: Binding<String> {
        Binding {
            if case .shortcut(let c) = action { c } else { "" }
        } set: { action = .shortcut($0) }
    }

    var body: some View {
        Picker(title, selection: kind) {
            Text("Toggle eraser").tag(Kind.eraser)
            Text("Show/hide toolbar").tag(Kind.toolbar)
            Text("Shortcut").tag(Kind.shortcut)
            Text("Nothing").tag(Kind.none)
        }
        if case .shortcut = action {
            ComboField(title: "\(title) keys", combo: combo)
        }
    }
}
