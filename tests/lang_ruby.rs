#![cfg(feature = "lang-ruby")]
//! End-to-end checks of the Ruby extractor and of how Ruby references are linked: a small
//! project is written to a temporary directory, indexed, and asked through the command line.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-ruby-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let p = root.join("src").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        Project { root }
    }

    fn ask(&self, args: &[&str]) -> String {
        let out = Command::new(BIN)
            .arg("--root")
            .arg(self.root.join("src"))
            .args(args)
            .env("CODEFAST_INDEX_DIR", self.root.join("index"))
            .output()
            .expect("run codefast");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn has(text: &str, needle: &str) {
    assert!(text.contains(needle), "expected `{needle}` in:\n{text}");
}

fn lacks(text: &str, needle: &str) {
    assert!(!text.contains(needle), "did not expect `{needle}` in:\n{text}");
}

/// A small application: a service with a mixin, a subclass, class methods, attributes, an
/// alias, a callback and a test.
fn app(name: &str) -> Project {
    Project::new(
        name,
        &[
            (
                "lib/shop/formatting.rb",
                "module Shop\n  module Formatting\n    def money(cents)\n      format_cents(cents)\n    end\n\n    def format_cents(cents)\n      cents / 100\n    end\n  end\nend\n",
            ),
            (
                "lib/shop/order.rb",
                "require_relative 'formatting'\n\nmodule Shop\n  class Order\n    include Formatting\n    attr_accessor :total, :status\n\n    def initialize(total)\n      @total = total\n      @status = :new\n    end\n\n    def summary\n      money total\n    end\n\n    def ship!\n      validate\n      self.status = :shipped\n    end\n    alias_method :dispatch!, :ship!\n\n    def self.build(total)\n      new(total)\n    end\n\n    class << self\n      def empty\n        build 0\n      end\n    end\n\n    private\n\n    def validate\n      raise ArgumentError if total.negative?\n    end\n  end\nend\n",
            ),
            (
                "lib/shop/rush_order.rb",
                "module Shop\n  class RushOrder < Order\n    def ship!\n      notify\n      super\n    end\n\n    def notify\n      count = 1\n      count + 1\n    end\n  end\nend\n",
            ),
            (
                "lib/shop/checkout.rb",
                "module Shop\n  class Checkout\n    def run(amount)\n      order = Order.new(amount)\n      order.ship!\n      order.total = amount + 1\n      puts order.summary\n      order.dispatch!\n      Order.empty\n      Order.build(3)\n    end\n  end\nend\n",
            ),
            (
                "app/controllers/orders_controller.rb",
                "class OrdersController < ApplicationController\n  before_action :load_order, only: [:show]\n\n  def show\n    render_order\n  end\n\n  private\n\n  def load_order\n    @order = Shop::Order.new(1)\n  end\n\n  def render_order\n    @order.summary\n  end\nend\n",
            ),
            ("app/controllers/application_controller.rb", "class ApplicationController\n  def self.before_action(*names, **opts)\n  end\nend\n"),
            (
                "test/order_test.rb",
                "require 'minitest/autorun'\n\nclass OrderTest < Minitest::Test\n  def test_summary\n    order = Shop::Order.new(250)\n    order.summary\n  end\nend\n",
            ),
        ],
    )
}

#[test]
fn ruby_definitions_are_found() {
    let p = app("ruby_definitions_are_found");
    let f = p.ask(&["find", "Order"]);
    has(&f, "lib/shop/order.rb:4");
    let s = p.ask(&["show", "Shop::Order.summary"]);
    has(&s, "def summary");
    // `def self.build` and the methods of `class << self` belong to the class.
    has(&p.ask(&["find", "build"]), "lib/shop/order.rb:23");
    has(&p.ask(&["find", "empty"]), "lib/shop/order.rb:28");
    has(&p.ask(&["find", "total"]), "lib/shop/order.rb:6");
}

#[test]
fn ruby_callers_through_a_receiver_built_with_new() {
    let p = app("ruby_callers_through_a_receiver_built_with_new");
    let c = p.ask(&["callers", "Order.summary"]);
    // `order = Order.new(..); order.summary` in another file, and `@order.summary` through an
    // instance variable assigned `Shop::Order.new`.
    has(&c, "lib/shop/checkout.rb");
    has(&c, "7 run: puts order.summary");
    has(&c, "app/controllers/orders_controller.rb");
    has(&c, "15 render_order: @order.summary");
}

#[test]
fn ruby_calls_without_parentheses() {
    let p = app("ruby_calls_without_parentheses");
    // `money total` and `build 0`: calls on `self` without parentheses.
    has(&p.ask(&["callers", "money"]), "14 summary: money total");
    has(&p.ask(&["callers", "build"]), "29 empty: build 0");
    let callees = p.ask(&["callees", "summary"]);
    has(&callees, "Formatting.money lib/shop/formatting.rb:3");
}

#[test]
fn ruby_module_method_through_include() {
    let p = app("ruby_module_method_through_include");
    let c = p.ask(&["callers", "format_cents"]);
    has(&c, "4 money: format_cents(cents)");
    has(&p.ask(&["callers", "Formatting.money"]), "lib/shop/order.rb");
    // The module lists the class that includes it.
    has(&p.ask(&["refs", "Formatting"]), "lib/shop/order.rb");
}

#[test]
fn ruby_inheritance_and_super() {
    let p = app("ruby_inheritance_and_super");
    // `super` in RushOrder#ship! calls Order#ship!.
    let c = p.ask(&["callers", "Order.ship!"]);
    has(&c, "5 ship!: super");
    has(&c, "5 run: order.ship!");
    has(&p.ask(&["refs", "Order"]), "lib/shop/rush_order.rb");
    has(&p.ask(&["callees", "RushOrder.ship!"]), "5 Order.ship! lib/shop/order.rb:17");
}

#[test]
fn ruby_class_methods() {
    let p = app("ruby_class_methods");
    has(&p.ask(&["callers", "Order.build"]), "10 run: Order.build(3)");
    has(&p.ask(&["callers", "Order.empty"]), "9 run: Order.empty");
    // `new(total)` inside the class and `Order.new(..)` elsewhere run `initialize`.
    let init = p.ask(&["callers", "Order.initialize"]);
    has(&init, "24 build: new(total)");
    has(&init, "4 run: order = Order.new(amount)");
}

#[test]
fn ruby_attr_accessor_across_files() {
    let p = app("ruby_attr_accessor_across_files");
    let r = p.ask(&["refs", "Order.total"]);
    has(&r, "6 run: order.total = amount + 1");
    has(&r, "14 summary: money total");
    has(&p.ask(&["refs", "Order.status"]), "19 ship!: self.status = :shipped");
    lacks(&r, "lib/shop/formatting.rb");
}

#[test]
fn ruby_callback_given_as_a_symbol() {
    let p = app("ruby_callback_given_as_a_symbol");
    let c = p.ask(&["callers", "load_order"]);
    has(&c, "orders_controller.rb");
    has(&c, "before_action() at app/controllers/orders_controller.rb:2");
    has(&p.ask(&["callees", "OrdersController"]), "load_order");
}

#[test]
fn ruby_alias_method() {
    let p = app("ruby_alias_method");
    // `order.dispatch!` calls `ship!` under its alias.
    let c = p.ask(&["callers", "Order.ship!"]);
    has(&c, "8 run: order.dispatch!");
    has(&p.ask(&["callees", "run"]), "8 Order.ship! lib/shop/order.rb:17");
}

#[test]
fn ruby_test_code_is_left_out_without_tests() {
    let p = app("ruby_test_code_is_left_out_without_tests");
    // Without --tests the call from the test is named apart, without its line.
    let without = p.ask(&["callers", "Order.summary"]);
    has(&without, "1 more in tests");
    has(&without, "test/order_test.rb: test_summary:6");
    lacks(&without, "6 test_summary: order.summary");
    let with = p.ask(&["callers", "Order.summary", "--tests"]);
    has(&with, "test/order_test.rb");
    has(&with, "6 test_summary: order.summary");
    has(&p.ask(&["find", "test_summary"]), "test/order_test.rb:4");
}

#[test]
fn ruby_locals_are_never_calls() {
    let p = app("ruby_locals_are_never_calls");
    // `count` in RushOrder#notify and `order` in Checkout#run are locals.
    let callees = p.ask(&["callees", "notify"]);
    lacks(&callees, "count");
    let run = p.ask(&["callees", "run"]);
    lacks(&run, "  amount");
    lacks(&run, " order ");
    // `validate` is a call on self (a private method), `total` an attribute.
    has(&p.ask(&["callers", "validate"]), "18 ship!: validate");
}

#[test]
fn ruby_private_methods_reach_subclasses_in_other_files() {
    let p = Project::new(
        "private",
        &[("base.rb", "class Base\n  private\n\n  def helper\n    1\n  end\nend\n"), ("child.rb", "class Child < Base\n  def go\n    helper\n  end\nend\n")],
    );
    has(&p.ask(&["callers", "helper"]), "3 go: helper");
}

#[test]
fn ruby_namespaces_tell_same_named_classes_apart() {
    let p = Project::new(
        "namespaces",
        &[
            ("a/base.rb", "module Alpha\n  class Base\n    def run\n      1\n    end\n  end\nend\n"),
            ("b/base.rb", "module Beta\n  class Base\n    def run\n      2\n    end\n  end\nend\n"),
            ("use.rb", "class User\n  def go\n    Alpha::Base.new.run\n  end\nend\n"),
            // `Admin` is not defined anywhere (Rails makes it from the directory).
            ("admin/thing.rb", "class Admin::Thing\n  def work\n    tidy\n  end\n\n  def tidy\n  end\nend\n"),
        ],
    );
    let alpha = p.ask(&["callers", "a/base.rb:3"]);
    has(&alpha, "3 go: Alpha::Base.new.run");
    let beta = p.ask(&["callers", "b/base.rb:3"]);
    lacks(&beta, "use.rb");
    has(&p.ask(&["callers", "tidy"]), "3 work: tidy");
}
