#![cfg(feature = "lang-js")]
//! Component files (`.vue`, `.svelte`, `.astro`) and the packages of a JavaScript workspace: a
//! small project is written to a temporary directory, indexed, and asked through the command
//! line, the way a harness would.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-components-{}-{name}", std::process::id()));
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

const MONEY: &str = "/** Format an amount of cents as a price. */
export function formatPrice(cents: number): string {
  return (cents / 100).toFixed(2)
}

export function useCart() {
  const items: number[] = []
  function add(n: number) { items.push(n) }
  return { items, add }
}
";

const PRICE_TAG: &str = "<template>
  <span class=\"price\" :title=\"label\">{{ formatPrice(cents) }}</span>
</template>

<script setup lang=\"ts\">
import { computed } from 'vue'
import { formatPrice } from '../lib/money'

const props = defineProps<{ cents: number }>()
const label = computed(() => `Price: ${formatPrice(props.cents)}`)
</script>

<style scoped>
.formatPrice { font-weight: bold }
</style>
";

const CART_VIEW: &str = "<script setup lang=\"ts\">
import PriceTag from './PriceTag.vue'
import { useCart } from '../lib/money'

const cart = useCart()

function addOne() {
  cart.add(100)
}

function clearAll() {
  cart.items.length = 0
}
</script>

<template>
  <div>
    <!-- <PriceTag /> in a comment is not a use, and neither is addOne -->
    <price-tag v-for=\"c in cart.items\" :cents=\"c\" />
    <PriceTag :cents=\"1\" />
    <button @click=\"addOne\">addOne</button>
    <button @click=\"clearAll()\">Clear all</button>
  </div>
</template>
";

const LEGACY: &str = "<template>
  <button @click=\"save\">{{ title }}</button>
</template>
<script>
import { formatPrice } from '../lib/money'
export default {
  data() { return { title: 'x' } },
  methods: {
    save() { return formatPrice(1) },
    unused() { return 2 },
  },
}
</script>
";

const COUNTER: &str = "<script lang=\"ts\">
  import { formatPrice } from '../lib/money'
  import Badge from './Badge.svelte'
  let count = 0
  function increment() { count += 1 }
  function tooltip(node: HTMLElement) { return {} }
</script>

<button on:click={increment} use:tooltip class=\"increment\">
  Clicked {count} times: {formatPrice(count)}
</button>
<Badge value={count} />
";

const PAGE: &str = "---
import Layout from '../components/Layout.astro'
import { formatPrice } from '../lib/money'
const prices = [100, 250]
function double(n: number) { return n * 2 }
---
<Layout title=\"Shop\">
  <ul>
    {prices.map((p) => <li>{formatPrice(double(p))}</li>)}
  </ul>
</Layout>
";

fn shop(name: &str) -> Project {
    Project::new(
        name,
        &[
            ("lib/money.ts", MONEY),
            ("components/PriceTag.vue", PRICE_TAG),
            ("components/CartView.vue", CART_VIEW),
            ("components/Legacy.vue", LEGACY),
            ("components/Counter.svelte", COUNTER),
            ("components/Badge.svelte", "<script>\n  export let value\n</script>\n<span>{value}</span>\n"),
            ("components/Layout.astro", "---\nconst { title } = Astro.props\n---\n<html><head><title>{title}</title></head><body><slot /></body></html>\n"),
            ("pages/index.astro", PAGE),
        ],
    )
}

#[test]
fn a_function_is_called_from_scripts_and_from_markup() {
    let p = shop("callers");
    let out = p.ask(&["callers", "formatPrice"]);
    has(&out, "all 5 call sites, 4 files");
    // In a script block, with the line it has in the file.
    has(&out, "10 label: const label = computed(() => `Price: ${formatPrice(props.cents)}`)");
    has(&out, "9 save: save() { return formatPrice(1) },");
    // In the markup of all three kinds of file.
    has(&out, "2 PriceTag: <span class=\"price\" :title=\"label\">{{ formatPrice(cents) }}</span>");
    has(&out, "10 Counter: Clicked {count} times: {formatPrice(count)}");
    has(&out, "pages/index.astro\n  9 ");
    // A style rule of the same name is not a call.
    lacks(&out, "font-weight");
}

#[test]
fn a_handler_named_in_the_markup_is_called_from_there() {
    let p = shop("handlers");
    // `@click="addOne"` calls it. The button's text and the comment do not.
    let out = p.ask(&["callers", "addOne"]);
    has(&out, "all 1 call sites, 1 files");
    has(&out, "21 CartView: <button @click=\"addOne\">addOne</button>");
    has(&p.ask(&["callers", "clearAll"]), "22 CartView: <button @click=\"clearAll()\">Clear all</button>");
    // Svelte: an event handler and an action.
    has(&p.ask(&["callers", "increment"]), "9 Counter: <button on:click={increment} use:tooltip class=\"increment\">");
    has(&p.ask(&["callers", "tooltip"]), "9 Counter: <button on:click={increment} use:tooltip class=\"increment\">");
    // The options API: a method of the component object, used by the template.
    has(&p.ask(&["callers", "save"]), "2 Legacy: <button @click=\"save\">{{ title }}</button>");
    has(&p.ask(&["search", "unused"]), "no callers found");
}

#[test]
fn a_component_is_a_symbol_and_its_tags_are_its_uses() {
    let p = shop("components");
    let out = p.ask(&["refs", "PriceTag"]);
    has(&out, "components/PriceTag.vue:1 ");
    has(&out, "19 CartView: <price-tag v-for=\"c in cart.items\" :cents=\"c\" />");
    has(&out, "20 CartView: <PriceTag :cents=\"1\" />");
    has(&out, "import PriceTag from './PriceTag.vue'");
    lacks(&out, "in a comment");
    // Svelte and Astro components the same way.
    has(&p.ask(&["refs", "Badge"]), "12 Counter: <Badge value={count} />");
    has(&p.ask(&["refs", "Layout"]), "<Layout title=\"Shop\">");
    // The outline of a component file: the component, then what its script declares.
    let outline = p.ask(&["show", "components/CartView.vue"]);
    has(&outline, "1-24\tCartView");
    has(&outline, "7-9\tfunction addOne()");
}

/// A workspace of two packages: `@shop/ui` uses `@shop/core` by its package name.
fn workspace(name: &str) -> Project {
    Project::new(
        name,
        &[
            ("package.json", "{\"name\": \"shop-monorepo\", \"private\": true, \"workspaces\": [\"packages/*\"]}\n"),
            ("packages/core/package.json", "{\"name\": \"@shop/core\", \"main\": \"./dist/index.js\", \"types\": \"./dist/index.d.ts\"}\n"),
            ("packages/core/src/index.ts", "export * from './price'\nexport { useData } from './data'\n"),
            ("packages/core/src/price.ts", "export function formatPrice(cents: number): string {\n  return String(cents)\n}\n"),
            ("packages/core/src/data.ts", "import { useData as useData$ } from 'some-framework'\n\nexport const useData: typeof useData$ = useData$\n"),
            ("packages/ui/package.json", "{\"name\": \"@shop/ui\", \"exports\": {\".\": {\"import\": \"./dist/index.mjs\"}}}\n"),
            ("packages/ui/src/index.ts", "export { render } from './render'\n"),
            (
                "packages/ui/src/render.ts",
                "import { formatPrice, useData } from '@shop/core'\nimport { formatPrice as direct } from '@shop/core/src/price'\n\nexport function render(cents: number) {\n  const { site } = useData()\n  return site + formatPrice(cents) + direct(cents)\n}\n",
            ),
            // Another function of the same name, which nothing imports.
            ("packages/ui/src/local.ts", "export function formatPrice(x: string) {\n  return x\n}\n"),
        ],
    )
}

#[test]
fn an_import_by_package_name_reaches_the_package() {
    let p = workspace("packages");
    let out = p.ask(&["callers", "packages/core/src/price.ts:1"]);
    // Both the import through the package's entry and the one through a path inside it.
    has(&out, "all 2 call sites, 1 files");
    has(&out, "6 render: return site + formatPrice(cents) + direct(cents)");
    lacks(&out, "by name");
    // The namesake in the other package is not what `@shop/core` exports.
    has(&p.ask(&["callers", "packages/ui/src/local.ts:1"]), "no callers found");
}

#[test]
fn a_call_of_a_constant_that_holds_a_function_from_another_package_is_a_call_of_it() {
    let p = workspace("foreign");
    let out = p.ask(&["refs", "useData"]);
    has(&out, "calls (1):");
    has(&out, "5 render: const { site } = useData()");
    lacks(&out, "name-only");
}
