final class Socket {
    func option(_ kind: Kind) -> Int {
        switch kind {
        case .timeout:
            return timeout()
        case .address:
            #if os(Windows)
            fallthrough
            #else
            return address()
            #endif
        default:
            return 0
        }
    }

    func timeout() -> Int { 1 }
    func address() -> Int { 2 }
}
