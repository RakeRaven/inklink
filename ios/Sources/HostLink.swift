import Network
import UIKit

/// Listens for the computer; the host connects in over Wi-Fi or through the usbmuxd USB tunnel.
final class HostLink {
    enum State {
        case waiting
        case connected(String)
    }

    var onState: ((State) -> Void)?
    var onHello: ((HostInfo) -> Void)?
    /// Called on the network queue.
    var onVideo: ((Data) -> Void)?

    private let queue = DispatchQueue(label: "inklink.net", qos: .userInteractive)
    private var listener: NWListener?
    private var connection: NWConnection?
    private var lastReceived = Date.distantPast
    private var bonjour: NetService?

    func start() {
        startListener()
        // Separate from the listener so a Bonjour refusal can't break USB or direct IP.
        let service = NetService(domain: "local.", type: Wire.bonjourType, name: UIDevice.current.name, port: Int32(Wire.port))
        service.publish()
        bonjour = service
    }

    private func startListener() {
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        let params = NWParameters(tls: nil, tcp: tcp)
        params.allowLocalEndpointReuse = true
        params.serviceClass = .interactiveVideo
        guard let listener = try? NWListener(using: params, on: NWEndpoint.Port(rawValue: Wire.port)!) else {
            queue.asyncAfter(deadline: .now() + 2) { self.startListener() }
            return
        }
        listener.newConnectionHandler = { [weak self] c in self?.accept(c) }
        listener.stateUpdateHandler = { [weak self] state in
            if case .failed = state {
                listener.cancel()
                self?.queue.asyncAfter(deadline: .now() + 2) { self?.startListener() }
            }
        }
        listener.start(queue: queue)
        self.listener = listener
    }

    private func accept(_ c: NWConnection) {
        // A live host keeps the iPad; a second one only takes over once the first goes quiet.
        if connection != nil, Date().timeIntervalSince(lastReceived) < 2 {
            c.cancel()
            return
        }
        connection?.cancel()
        connection = c
        lastReceived = Date()
        let via: String
        if case .hostPort(let host, _) = c.endpoint, "\(host)".hasPrefix("127.") || "\(host)" == "::1" {
            via = "USB"
        } else {
            via = "Wi-Fi"
        }
        c.stateUpdateHandler = { [weak self] state in
            guard let self, c === self.connection else { return }
            switch state {
            case .ready:
                c.send(content: Message.hello(), completion: .contentProcessed { _ in })
                self.readHeader(c)
                DispatchQueue.main.async { self.onState?(.connected(via)) }
            case .failed, .cancelled:
                self.connection = nil
                DispatchQueue.main.async { self.onState?(.waiting) }
            default:
                break
            }
        }
        c.start(queue: queue)
    }

    private func readHeader(_ c: NWConnection) {
        c.receive(minimumIncompleteLength: 5, maximumLength: 5) { [weak self] data, _, _, error in
            guard let self, let data, data.count == 5, error == nil else { c.cancel(); return }
            let b = [UInt8](data)
            let len = Int(UInt32(b[1]) | UInt32(b[2]) << 8 | UInt32(b[3]) << 16 | UInt32(b[4]) << 24)
            guard len <= 16 << 20 else { c.cancel(); return }
            if len == 0 {
                self.handle(b[0], Data())
                self.readHeader(c)
                return
            }
            c.receive(minimumIncompleteLength: len, maximumLength: len) { payload, _, _, error in
                guard let payload, payload.count == len, error == nil else { c.cancel(); return }
                self.handle(b[0], payload)
                self.readHeader(c)
            }
        }
    }

    private func handle(_ kind: UInt8, _ payload: Data) {
        lastReceived = Date()
        switch kind {
        case Wire.video:
            onVideo?(payload)
        case Wire.hostHello:
            if let info = try? HostInfo(payload) {
                DispatchQueue.main.async { self.onHello?(info) }
            }
        default:
            break
        }
    }

    func send(_ data: Data) {
        queue.async { self.connection?.send(content: data, completion: .contentProcessed { _ in }) }
    }

    /// The iPad's Wi-Fi address, shown while waiting so the host can use --host.
    static func wifiAddress() -> String? {
        var ifaddr: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&ifaddr) == 0, let first = ifaddr else { return nil }
        defer { freeifaddrs(ifaddr) }
        for p in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let ifa = p.pointee
            guard let addr = ifa.ifa_addr, addr.pointee.sa_family == UInt8(AF_INET),
                  String(cString: ifa.ifa_name) == "en0" else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            if getnameinfo(addr, socklen_t(addr.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
                return String(cString: host)
            }
        }
        return nil
    }
}
