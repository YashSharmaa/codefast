#![cfg(feature = "lang-kotlin")]
//! End-to-end checks of Kotlin: a small project is written to a temporary directory, indexed,
//! and asked through the command line, the way a harness would. One project mixes Kotlin and
//! Java in the same packages.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-kotlin-{}-{name}", std::process::id()));
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

const ORDER: &str = "package com.acme.shop

data class Order(val id: Int, var name: String) {
    fun total(): Int = id * 2

    companion object {
        fun create(id: Int): Order = Order(id, \"new\")
    }
}

interface Repo {
    fun find(id: Int): Order
}

class MemRepo : Repo {
    override fun find(id: Int): Order = Order.create(id)
}

class CachedRepo(private val inner: Repo) : Repo {
    override fun find(id: Int): Order = inner.find(id)
}
";

const SERVICE: &str = "package com.acme.shop

import com.acme.util.slugify
import com.acme.legacy.LegacyBilling

fun Order.describe(): String = name + total()

class Service(private val repo: Repo) {
    fun handle(id: Int): String {
        val order = repo.find(id)
        val label = order.describe()
        val billing = LegacyBilling()
        billing.charge(order.total())
        return slugify(order.name) + label
    }
}
";

const STRINGS: &str = "package com.acme.util

fun slugify(s: String): String = s.lowercase()

fun String.shout(): String = uppercase()
";

const LEGACY: &str = "package com.acme.legacy;

import com.acme.shop.Order;
import com.acme.shop.Service;

public class LegacyBilling {
    public void charge(int amount) {
        Order o = Order.Companion.create(amount);
        o.total();
    }

    public String run(Service s) {
        return s.handle(1);
    }
}
";

const MAIN: &str = "package com.acme.app

import com.acme.shop.MemRepo
import com.acme.shop.Order
import com.acme.shop.Service
import com.acme.util.shout
import com.acme.util.slugify

fun main() {
    val s = Service(MemRepo())
    println(s.handle(1).shout())
    val o = Order.create(7)
    o.name = slugify(o.name)
}
";

// Not under a test directory and not named like a test: only the annotation says it is one.
const CHECKS: &str = "package com.acme.checks

import com.acme.shop.Order
import org.junit.jupiter.api.Test

class OrderChecks {
    @Test
    fun totalIsDoubled() {
        Order.create(2).total()
    }
}
";

fn shop(name: &str) -> Project {
    Project::new(
        name,
        &[
            ("com/acme/shop/Order.kt", ORDER),
            ("com/acme/shop/Service.kt", SERVICE),
            ("com/acme/util/Strings.kt", STRINGS),
            ("com/acme/legacy/LegacyBilling.java", LEGACY),
            ("com/acme/app/Main.kt", MAIN),
            ("checks/OrderChecks.kt", CHECKS),
        ],
    )
}

#[test]
fn definitions_are_found_and_shown() {
    let p = shop("defs");
    let find = p.ask(&["find", "Order"]);
    has(&find, "com/acme/shop/Order.kt:3 data class Order(val id: Int, var name: String)");
    has(&p.ask(&["show", "Order.create"]), "fun create(id: Int): Order = Order(id, \"new\")");
    // A companion's members belong to the class, a data class's `val`s are its fields.
    let ctx = p.ask(&["context", "Order.total"]);
    has(&ctx, "member of data class Order");
    has(&ctx, "fun create(id: Int): Order");
    has(&ctx, "val id: Int");
    has(&p.ask(&["find", "--kind", "fn", "describe"]), "com/acme/shop/Service.kt:6 fun Order.describe(): String");
}

#[test]
fn callers_across_files_through_a_typed_receiver() {
    let p = shop("typed");
    // `val s = Service(MemRepo())` types `s`; the call is in another package.
    let callers = p.ask(&["callers", "Service.handle"]);
    has(&callers, "com/acme/app/Main.kt");
    has(&callers, "11 main: println(s.handle(1).shout())");
    // `repo` is a constructor property declared with its type.
    has(&p.ask(&["callers", "Order.total"]), "13 handle: billing.charge(order.total())");
    has(&p.ask(&["path", "main", "Order.create"]), "com/acme/app/Main.kt:12 main calls create");
}

#[test]
fn interface_calls_reach_the_implementations() {
    let p = shop("dispatch");
    let callers = p.ask(&["callers", "MemRepo.find"]);
    has(&callers, "10 handle: val order = repo.find(id)");
    has(&callers, "20 find: override fun find(id: Int): Order = inner.find(id)");
    has(&p.ask(&["callees", "handle"]), "2 impls: MemRepo.find, CachedRepo.find");
}

#[test]
fn extension_functions_are_members_of_their_receiver() {
    let p = shop("ext");
    // An extension of a type of this repository, called on a typed local.
    let callers = p.ask(&["callers", "describe"]);
    has(&callers, "all 1 call sites, 1 files");
    has(&callers, "11 handle: val label = order.describe()");
    // An extension of `String`, called on the declared result of a method.
    has(&p.ask(&["callers", "shout"]), "11 main: println(s.handle(1).shout())");
    // An implicit-receiver call inside an extension body.
    has(&p.ask(&["callers", "Order.total"]), "6 describe: fun Order.describe(): String = name + total()");
}

#[test]
fn companion_object_calls_go_through_the_class() {
    let p = shop("companion");
    let callers = p.ask(&["callers", "create"]);
    has(&callers, "16 find: override fun find(id: Int): Order = Order.create(id)");
    has(&callers, "12 main: val o = Order.create(7)");
}

#[test]
fn top_level_functions_imported_from_another_package() {
    let p = shop("toplevel");
    let callers = p.ask(&["callers", "slugify"]);
    has(&callers, "all 2 call sites, 2 files");
    has(&callers, "13 main: o.name = slugify(o.name)");
    has(&callers, "14 handle: return slugify(order.name) + label");
    has(&p.ask(&["callees", "handle"]), "slugify com/acme/util/Strings.kt:3");
}

#[test]
fn kotlin_and_java_call_each_other() {
    let p = shop("interop");
    // Kotlin calls a Java method through a local typed by a Java constructor.
    has(&p.ask(&["callers", "charge"]), "13 handle: billing.charge(order.total())");
    has(&p.ask(&["callees", "handle"]), "LegacyBilling.charge com/acme/legacy/LegacyBilling.java:7");
    // Java calls a Kotlin method, and a Kotlin companion member through `Companion`.
    has(&p.ask(&["callers", "Service.handle"]), "13 run: return s.handle(1);");
    has(&p.ask(&["callers", "create"]), "8 charge: Order o = Order.Companion.create(amount);");
    has(&p.ask(&["callers", "Order.total"]), "9 charge: o.total();");
    has(&p.ask(&["refs", "Order"]), "com/acme/legacy/LegacyBilling.java");
}

#[test]
fn data_class_properties_are_used_across_files() {
    let p = shop("props");
    let refs = p.ask(&["refs", "Order.name"]);
    has(&refs, "writes (1):");
    has(&refs, "13 main: o.name = slugify(o.name)");
    has(&refs, "14 handle: return slugify(order.name) + label");
    // Inside the extension, `name` is the receiver's property.
    has(&refs, "6 describe: fun Order.describe(): String = name + total()");
}

#[test]
fn test_functions_are_test_code() {
    let p = shop("tests");
    // Without --tests the call from the test is named apart, without its line.
    let callers = p.ask(&["callers", "create"]);
    has(&callers, "1 more in tests");
    has(&callers, "checks/OrderChecks.kt: totalIsDoubled:9");
    lacks(&callers, "Order.create(2).total()");
    let with_tests = p.ask(&["callers", "--tests", "create"]);
    has(&with_tests, "9 totalIsDoubled: Order.create(2).total()");
}

#[test]
fn same_file_top_level_functions_and_same_package_classes() {
    let p = Project::new(
        "samefile",
        &[
            (
                "a/Widgets.kt",
                "package a\n\nfun render(w: Widget): String = w.title\n\nclass Panel(val widgets: List<Widget>) {\n    fun draw(): String {\n        fun join(xs: List<String>) = xs.joinToString()\n        return join(widgets.map { render(it) })\n    }\n}\n",
            ),
            ("a/Widget.kt", "package a\n\nclass Widget(val title: String)\n"),
            ("b/Use.kt", "package b\n\nimport a.Panel\nimport a.Widget\n\nfun use() = Panel(listOf(Widget(\"t\"))).draw()\n"),
        ],
    );
    // A top-level function of the same file, called from inside a class.
    has(&p.ask(&["callers", "render"]), "8 draw: return join(widgets.map { render(it) })");
    // A local function.
    has(&p.ask(&["callers", "join"]), "8 draw: return join(widgets.map { render(it) })");
    // A class of the same package needs no import; a constructor call names the class.
    has(&p.ask(&["refs", "Widget"]), "a/Widgets.kt");
    has(&p.ask(&["callers", "Widget"]), "6 use: fun use() = Panel(listOf(Widget(\"t\"))).draw()");
    has(&p.ask(&["callers", "Panel.draw"]), "b/Use.kt");
}
