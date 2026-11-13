# Definitions: namespaces, classes, methods of every form, attributes, constants, structs.
module Shop
  VERSION = "1.0"
  Money = Struct.new(:cents, :currency) do
    def to_s
      "#{cents} #{currency}"
    end
  end
  Point = Data.define(:x, :y)

  class Order < Base
    attr_reader :total
    attr_writer :note
    attr_accessor :status, :owner

    LIMIT = 10

    def initialize(total, note = nil, *rest, rush:, gift: false, **opts, &block)
      @total = total
      @items = Items.new
    end

    def self.build(total) = new(total)

    class << self
      def empty
        build(0)
      end
    end

    def name=(value)
      @name = value
    end

    def ready?
      true
    end

    def check!
      raise NotImplementedError
    end

    alias summary to_s
    alias_method :finish!, :check!
    define_method(:twice) { |n| n * 2 }

    private

    def hidden
      1
    end

    public

    def visible
      2
    end

    private def secret
      3
    end

    def other
      4
    end
    private :other
  end

  class Order
    def reopened
      5
    end
  end

  class Admin::Report < ::Shop::Order
  end
end

def top_level(a, b)
  a + b
end
