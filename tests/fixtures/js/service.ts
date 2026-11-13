import DefaultExport, { helper as renamedHelper, type Options, Emitter } from './base';
import * as util from '../common/util';
import fs = require('fs');
import './polyfill';
import type { Disposable } from "./lifecycle";
import Token = util.tokens.Token;
export { first, second as other } from './reexport';
export * from './everything';
export * as nested from './nested';

export const MAX_RETRIES = 3;
export const defaultOptions: Options = { retries: MAX_RETRIES, log: logMessage };
const registry = new Registry<Widget>();
let counter = 0, lastWidget: Widget | undefined;
export const IWidgetService = createDecorator<IWidgetService>('widgetService');

export interface IWidgetService extends Disposable, util.Service<Widget> {
	readonly _brand: undefined;
	current?: Widget;
	onDidChange: (e: WidgetEvent) => void;
	create(name: string, options?: Options): Widget;
	get count(): number;
	[key: string]: unknown;
}

export type WidgetMap<T extends Widget = Widget> = Map<string, T> | undefined;
type Handler = (widget: Widget, ...rest: unknown[]) => boolean;
type Shape = { kind: Kind; area(): number };

export const enum Kind { Small = 1, Large, 'Extra' = 4 }
enum Color { Red, Green = Red + 1 }

export abstract class BaseService<T> implements Disposable {
	protected abstract readonly id: string;
	static instances = 0;
	abstract start(delay: number): Promise<void>;
	dispose(): void { BaseService.instances--; }
}

@registerSingleton(IWidgetService)
export class WidgetService extends BaseService<Widget> implements IWidgetService, util.Service<Widget> {
	declare readonly _brand: undefined;
	protected readonly id = 'widgets';
	private widgets: WidgetMap = new Map();
	current?: Widget;
	#secret = 0;
	onDidChange = (e: WidgetEvent): void => { this.logger.trace(e.kind); };
	private fallback: Handler = defaultHandler;

	constructor(
		@ILogService private readonly logger: ILogService,
		private readonly emitter: Emitter<WidgetEvent>,
		@optional(IStorage) protected storage?: util.Storage,
		retries = MAX_RETRIES,
	) {
		super();
		registry.register(this);
	}

	get count(): number { return this.widgets!.size; }
	set count(value: number) { counter = value; }

	create(name: string): Widget;
	create(name: string, options: Options): Widget;
	create(name: string, options?: Options): Widget {
		const widget = new Widget(name, options ?? defaultOptions);
		Widget.validate(name, Emitter.None);
		const copy: Widget = widget.clone();
		copy.render(Kind.Large);
		this.widgets?.set(name, widget);
		this.emitter.fire({ kind: Kind.Small, widget });
		this.current = widget;
		lastWidget = copy;
		counter += 1;
		return renamedHelper(widget) as Widget;
	}

	@memoize
	async start(delay: number, ...extra: string[]): Promise<void> {
		await util.timeout(delay);
		await super.dispose();
		const self = this;
		const { widgets, current } = this;
		this.storage?.items.forEach(function (item) { self.restore(item); });
		for (const [key, value] of this.widgets ?? []) { value.render(Kind.Small); }
		try { fs.readFileSync(DefaultExport.path); } catch (err) { this.logger.error(err); }
		WidgetService.instances++;
		this.emitter.listener = handleEvent;
	}

	private restore<K extends keyof Shape>(item: Shape, key?: K): item is Shape {
		function visit(shape: Shape): number { return shape.area() + visit(shape); }
		const measure = (s: Shape) => visit(s);
		return measure(item) > 0 && isShape<Shape>(item);
	}

	static *ids(): Iterable<string> { yield* registry.keys(); }
}

export function logMessage(message: string, level = 0, ...args: unknown[]): void {
	console.log(message, level, args);
}
function defaultHandler(widget: Widget): boolean { return widget instanceof Widget; }
function handleEvent(this: WidgetService, e: WidgetEvent) {}
export const makeHandler = async <T extends Widget>(w: T, strict?: boolean): Promise<Handler> => defaultHandler;
const legacyHandler = function named(a, b) { return named(b, a); };

export namespace Widgets.Internal {
	export const cache = new Map<string, Widget>();
	export function lookup(id: string): Widget | undefined { return cache.get(id); }
	function prune() { cache.clear(); }
}
namespace tools { export function format(w: Widget) { return w.name; } }
tools.format(lastWidget!);
Widgets.Internal.lookup('a');

export const api = {
	version: '1.0',
	open(path: string) { return this.close(path); },
	close: (path: string) => util.fs.unlink(path),
	onError: handleEvent,
	nested: { deep() { return counter; } },
	logMessage,
};
api.open('x');

export default WidgetService;
export { registry as widgetRegistry, tools };
