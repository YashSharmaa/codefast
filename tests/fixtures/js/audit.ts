// Cases found by the 2026-11-13 audit: grammar quirks around calls, hoisted locals, type-level names.
import { Kind, type Box, tools as kit } from './k';
import * as ns from './ns';
import legacy = require('./legacy');

export class Loader {
	constructor(private readonly svc: Service) {}
	load<T>(n: number): Promise<Loaded> { return null!; }
	check<T>(n: number): boolean { return true; }
	count<T>(n: number): number { return 1; }

	// Explicit type arguments after a prefix or binary operator: the grammar reads the callee as
	// `await this.load`, `!this.check`, `1 + this.count`.
	async run() {
		const m = await this.load<Foo>(13);
		m.go();
		if (!this.check<Foo>(1)) { return; }
		const k = 1 + this.count<Foo>(2);
		const v = void free<Foo>(3);
		const w = typeof ns.make<Foo>(4);
		const neg = !!kit.test<Foo>(5);
		(await this.svc.fetch<Bar>(6)).done();
		return await this.svc.fetch<Bar>(7);
	}

	// Parenthesised callees, and a module binding called as a function.
	paren(obj: Service) {
		(free)(1);
		(obj.fetch)(2);
		legacy(3);
		new legacy();
	}
}

// `let` / `const` are visible in their whole block: a closure may use one declared further down.
function closures(view: View) {
	const d = view.onDidChange(size => handle(item, size, later.go()));
	const item = make();
	const later: Later = pick();
	{
		use(() => inner);
		const inner = 1;
	}
	const fn = () => item;
}

// A type parameter may be used in the constraint of an earlier one; `Kind.Member` and
// `Outer.Inner` in type position use the enum / namespace as well.
interface Shape {
	log<E extends Box<T> = never, T extends Kind.Member = never>(e: E): void;
	inner: Outer.Inner;
	viaNs: ns.Thing;
}

// String keys: names of members only when they could be referred to by name.
const cases = {
	'a cancel while draining': (support: Support) => { support.go(); },
	'content-type': () => 1,
	'CallExpression[callee.name="x"]': (node: Node) => { report(node); },
	plain: () => 2,
};
