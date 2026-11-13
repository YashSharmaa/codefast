# Test code: every definition here is test code.
require "test_helper"

class OrderTest < ActiveSupport::TestCase
  setup :prepare

  test "ships an order" do
    order = Order.new(1)
    order.ship
  end

  def test_total
    assert_equal 1, Order.new(1).total
  end

  def prepare
  end
end
