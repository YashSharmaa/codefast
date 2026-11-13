// Cases found by the 2026-11-13 audit (JavaScript grammar).
const glob = require('glob');
const Mocha = require('mocha');
const helper = require('./helper');

// `await (expr)(args)` is read as a call of `await`.
async function runOnce() {
	const metrics = {
		delta: await (async () => measure())(),
		other: await (fetchIt)(1),
		third: await (pending),
	};
	return metrics;
}

// A module binding called or constructed is the export of that module.
function tests() {
	const local = require('./local');
	glob('**/*.js', {}, () => {});
	const mocha = new Mocha({ ui: 'tdd' });
	helper(1);
	helper.sub(2);
	local(3);
	return mocha;
}

function hoisted() {
	on(() => later.run());
	const later = create();
}

// `bind` / `call` of a module alias is a function of that module, not `Function.prototype.bind`.
function Server() {}
Server.prototype.listen = function (port) { return helper.bind(this, port); };
Server.prototype.close = function () { return finish.call(this, 1); };
