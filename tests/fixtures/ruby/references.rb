# References: calls with and without parentheses, receivers, super, constants.
require "json"
require_relative "support/helpers"
load "tasks.rb"
autoload :Parser, "shop/parser"

class Cart < Shop::Basket
  def initialize(items)
    super
    @items = items
    @parser = Shop::Parser.new
  end

  def add(item, qty = 1)
    super(item)
    validate item
    validate(item)
    log
    self.count += qty
    @items << item
    @parser.parse(item)
    Shop::Pricing.price(item)
    Shop::Pricing::TAX
    ::Logger.new
  end

  def receivers
    c = Cart.new([])
    c.add 1
    d = c
    d.add(2)
    n = nil
    n = Order.new
    n.ship
    s = "x"
    s.shout
    [1].first
    self.class.make
    Cart.new.total.round
    x = 1
    x = Order.new
    x.ship
  end
end

top_level 1, 2
