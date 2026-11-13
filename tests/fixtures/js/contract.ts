// Rules of the 2026-11-13 contract update: receiver type paths, module-qualified names, imports.
import * as core from '../core/index.js';
import * as ns from './factory';
import type * as types from './types';
import Default, { thing as renamed, Registry } from './things';
import legacy = require('./legacy');
import { Emitter as Evt, type Options as Opts, tools as kit } from './events';
export * as schemas from './schemas';
export { a as b, default as c } from './abc';
export { default } from './d';
export * from './all';
const cjs = require('./cjs');
const { pick, omit: without } = require('./cjs-named');
const viaMember = require('./cjs-member').member;

const z = { ...core, string() { return this.parse(); }, parse: core.parse, check: handleThing };
const lazy = await import('./lazy');
const { helper, other: alias2 } = await import('./helpers');
const made = make();
const stringType = ZodString.create;
const viaNs = ns.build;
const plain = handleThing;
const joined = core.paths.join.bind(core.paths);
export const coerce = { text: ((arg) => ZodString.create(arg)) as Maker, raw: <Maker>function (arg) { return arg; } };
type Shape = { kind: 'a'; a: number } | ({ kind: 'b'; b(): void } & Extra) | { kind: 'c' };
namespace local { export const table = new Map<string, number>(); export function helper(): void {} }
let current: core.$ZodType | undefined;
export const registry: core.$ZodRegistry<Meta> = core.registry();

export class ZodType extends core.$ZodType implements types.Schema, ns.inner.Shape {
	static create = makeType;
	static alt = ZodString.create;
	def: core.$ZodTypeDef;
	private readonly evt: Evt<Opts> = new Evt();
	constructor(private readonly checks: core.$ZodCheck[], readonly inner: types.deep.Inner) { super(); }
	nullable(): ZodNullable { return new ZodNullable(this); }
	optional(): core.$ZodOptional { return new core.$ZodOptional(this); }
	nullish() { return this.nullable().optional(); }
	async load(): Promise<ZodType> { return this; }
	async run(x: core.$ZodType): Promise<types.Result> {
		foo().bar();
		ns.make().run();
		core.globalRegistry.get(x);
		core.util.defineLazy(x);
		core.Registry.create().add(x);
		z.string();
		made.use();
		current.touch();
		stringType().optional();
		viaNs().done();
		plain();
		const loaded = await this.load();
		loaded.nullish();
		const r = this.nullable();
		r.unwrap();
		const { status } = this.process(x);
		status.dirty();
		(await ns.fetch()).json();
		lazy.start();
		helper();
		alias2();
		legacy.old();
		cjs.run();
		pick(without(viaMember()));
		renamed();
		new core.$ZodError([]);
		const e = new ns.inner.Thing();
		e.go();
		x.check();
		this.inner.value.touch();
		super.run(x).finish();
		Default.configure();
		Registry.global.get(x);
		this.evt.fire(Evt.None, kit.wrap(x));
		this["~validate"](x);
		local.table.get('k');
		local.helper();
		joined('a', 'b');
		const inline: { run(): void; stop: () => void } = make();
		inline.run();
		const run = this.nullish;
		run();
		return x as types.Result;
	}
}
function makeType() {}
function handleThing() {}
async function lazyLoad() {
	const m = await import('./lazy');
	const { pick: p2, omit } = require('./cjs-named');
	m.start(p2(omit()));
}
