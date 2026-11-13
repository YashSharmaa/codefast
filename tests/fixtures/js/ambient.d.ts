/// <reference types="node" />
declare const VERSION: string;
declare let mutableFlag: boolean, lastError: AppError | undefined;
declare function overloaded(value: string): string;
declare function overloaded(value: number, radix?: number): string;
declare class AmbientWidget extends Base {
	constructor(name: string);
	readonly name: string;
	render(target: Element, ...children: AmbientWidget[]): void;
	static create(): AmbientWidget;
}
declare namespace Platform.Paths {
	const sep: string;
	function join(...parts: string[]): string;
	interface Parsed { dir: string; base: string }
}
declare module 'virtual-module' {
	import { Disposable } from 'lifecycle';
	export function activate(context: Context): Disposable;
	export const enabled: boolean;
	export default class Plugin { dispose(): void; }
}
declare global {
	interface Window { appState: AppState }
	var __DEV__: boolean;
}
declare enum Mode { Read, Write }
interface AppState { mode: Mode; widgets: AmbientWidget[]; on(event: 'change', cb: (s: AppState) => void): void }
type Callback<T = void> = (error: AppError | null, value?: T) => void;
export as namespace AppLib;
export = Platform;
