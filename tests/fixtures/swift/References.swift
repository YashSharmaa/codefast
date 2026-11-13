import Foundation
import NIOCore

final class Controller: Base {
    let session = Session()
    var state: State = .idle
    var registry: Registry?
    var onDone: (() -> Void)?

    func run(url: URL, helper: Helper) async throws {
        let s = Session()
        s.request(url).validate()
        let h: Helper = Helper.make(1)
        h.go()
        helper.go()
        registry?.lookup(1)
        registry!.lookup(2)
        reload()
        self.reload()
        super.prepare()
        Controller.factory()
        Self.factory()
        session.task.resume()
        self.session.start()
        let result = fetch().decode()
        let buffer = NIOCore.ByteBuffer()
        let channel: NIOCore.Channel? = nil
        channel?.close()
        _ = buffer
        try await s.download(url) { progress in
            progress.report()
        }
        s.upload(url) { $0.done() } completion: { _ in }
        _ = result
    }

    func callbacks(button: Button, list: [Int]) {
        button.onTap = handle
        button.onTap = self.handle
        onDone = finish
        _ = list.map(transform)
        _ = list.map(Controller.convert)
        register(handler: handle)
        let sel = #selector(Controller.handle(_:))
        button.addTarget(self, action: #selector(tapped), for: .touchUpInside)
        _ = sel
        let names = items.map(\.name)
        let path = \Item.name.count
        _ = names
        _ = path
    }

    func contextual() -> State {
        state = .running
        if state == .idle { }
        switch state {
        case .idle: break
        case .running: break
        case .failed(let error): error.log()
        }
        let next: State = .init(raw: 1)
        let other = State.make()
        let p = Point(x: 1, y: 2)
        let q = Point.init(x: 1, y: 2)
        _ = (next, other, p, q)
        return .idle
    }

    func casts(value: Any) {
        if let c = value as? Circle { c.draw() }
        let forced = value as! Square
        forced.draw()
        if value is Triangle {}
        let kind = Kind.alpha
        _ = kind
        print(globalConstant)
        globalVariable.append("x")
        freeFunction(1)
        _ = String(describing: value).shout()
    }

    func handle(_ v: Int) {}
    func finish() {}
    func tapped() {}
    static func factory() {}
    static func convert(_ v: Int) -> Int { v }
}

func transform(_ v: Int) -> Int { v }
func fetch() -> Response { Response() }

let topLevel = Controller()
topLevel.reload()
