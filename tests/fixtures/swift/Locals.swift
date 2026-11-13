func scopes(items: [Item], maybe: Item?, table: [String: Item]) throws {
    let local = Item()
    local.use()
    var counter = 0
    counter += 1
    for item in items {
        item.use()
    }
    for (index, item) in items.enumerated() {
        item.use(index)
    }
    if let found = maybe {
        found.use()
    }
    if let maybe {
        maybe.use()
    }
    guard let first = items.first else { return }
    first.use()
    while let next = iterator.next() {
        next.use()
    }
    if case .some(let wrapped) = maybe {
        wrapped.use()
    }
    table["k"]?.use()
    items[0].use()
    do {
        try risky()
    } catch let failure as NetworkError {
        failure.retry()
    } catch {
        error.report()
    }
    let closure = { (a: Item, b) in
        a.use()
        b.use()
    }
    items.forEach { $0.use() }
    items.forEach { [weak delegate, captured = local] entry in
        captured.use()
        entry.use()
        delegate?.done()
    }
    _ = closure
    unknownGlobal.use()
}

struct Holder {
    var value: Item
    func work() {
        value.use()
        let value = Other()
        value.use()
    }
}
