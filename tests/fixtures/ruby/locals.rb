# Locals: a bound name never becomes a call; an unbound one is a call.
class Locals
  def run(a, (b, c), *d, e:, **f, &g)
    a; b; c; d; e; f; g
    h = 1
    h
    before
    before = 2
    before
    i, (j, k), *l = list
    i; j; k; l
    m = m
    items.each { |x, y = x; z| x; y; z; it }
    items.each { it.go }
    items.each { _1.go }
    items.each do |p|
      q = p
    end
    q
    for r in rows do r.go end
    r
    begin
      work
    rescue Error => err
      err.message
    end
    case value
    in { name: String => nm, age: } then nm; age
    in [Integer => first, *rest] then first; rest
    in ^h then 1
    end
    value => { id: }
    id
    fn = ->(t) { t; u }
    fn
    defined?(maybe)
    w ||= 5
    w
    { h:, missing: }
  end
end
