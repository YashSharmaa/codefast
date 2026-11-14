# stencil: a small text template engine

`stencil` renders text templates with variables, filters, conditions, loops and includes, and
escapes HTML by default. It is a TypeScript library with a command line. The project will be
checked by tests written against the interface below, so names, signatures, error classes and
output must match exactly.

## Runtime and module rules

The code is TypeScript run directly by Node 26, which strips types. There is no build step and
no dependency. Relative imports carry the `.ts` extension (`import { Environment } from
"./environment.ts"`). Only erasable syntax is allowed: no `enum`, no `namespace`, no
constructor parameter properties, no `<T>value` type assertions (write `value as T`), and
type-only imports use `import type` or `import { type X }`. Files are ES modules. A
`package.json` is optional and, if present, has `"type": "module"` and no dependencies.

## Files and exports

| File | Exports |
|---|---|
| `src/errors.ts` | `TemplateError` (extends `Error`), `TemplateSyntaxError` and `RenderError` (both extend `TemplateError`). `TemplateSyntaxError` has `line: number` (1-based) and is built as `new TemplateSyntaxError(message, line)`, the others as `new X(message)`. |
| `src/filters.ts` | `type Filter = (value: unknown, ...args: unknown[]) => unknown` and `builtinFilters: Record<string, Filter>` |
| `src/environment.ts` | `class Environment` and `type EnvironmentOptions` |
| `src/cli.ts` | the command line, run as `node src/cli.ts ...` |

```ts
type EnvironmentOptions = {
  templates?: Record<string, string>; // parsed by the constructor
  loader?: (name: string) => string | undefined; // asked for names not in templates
  strict?: boolean; // default false
  autoescape?: boolean; // default true
};
class Environment {
  constructor(options?: EnvironmentOptions);
  addTemplate(name: string, source: string): void; // parses now
  addFilter(name: string, fn: Filter): void; // replaces a built-in of the same name
  render(name: string, data?: Record<string, unknown>): string;
  renderString(source: string, data?: Record<string, unknown>): string;
}
```

`TemplateSyntaxError` comes from parsing: the constructor, `addTemplate`, `renderString`, and
templates fetched through `loader`. Everything that fails while rendering is a `RenderError`,
including `render` of a name that neither `templates` nor `loader` knows.

## Template syntax

Text outside tags is copied unchanged. `{{ expr }}` outputs a value, `{% ... %}` is a
statement, `{# ... #}` is a comment and outputs nothing. A `-` right inside a delimiter
(`{{-`, `-}}`, `{%-`, `-%}`, `{#-`, `-#}`) removes all whitespace (spaces, tabs, newlines)
between the tag and the previous or next non-whitespace text.

Statements:

- `{% if expr %} ... {% elif expr %} ... {% else %} ... {% endif %}` (any number of `elif`, one optional `else` last)
- `{% for x in expr %} ... {% else %} ... {% endfor %}` and `{% for k, v in expr %}`. The
  `else` part runs when there is nothing to iterate.
- `{% include "name" %}` renders another template (the name is a string literal).
- `{% raw %} ... {% endraw %}` outputs its content literally, tags included.

A `TemplateSyntaxError` reports the line on which the offending tag starts. Cases: a tag
without its closing delimiter, an unknown statement, `elif`, `else`, `endif` or `endfor`
without a matching open block, a second `else`, `elif` after `else`, an empty `{{ }}`, a
malformed expression or `for`, an `include` without a string literal, and a block that is never
closed (reported at the line of its opening tag).

## Expressions

From loosest to tightest: `or`, `and`, `not`, comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`,
`in`, not chained), filters (`value | name` or `value | name(arg, ...)`, applied left to
right), and atoms: string literals in single or double quotes (a backslash makes the next
character literal, and `\n` is a newline), numbers (`-?[0-9]+(\.[0-9]+)?`), `true`, `false`,
`null`, variable paths and parentheses.

A path is a name followed by `.segment` parts (`user.name`, `items.0`). On an array, a segment
of digits picks that element and nothing else is found (so `items.length` is `undefined`). On
another non-null object a segment picks an own property. Anything else, including a missing
top-level name, is `undefined`. Inside a `for`, the loop variables and `loop` hide outer
names of the same name until the loop ends.

- Falsy values: `false`, `null`, `undefined`, `0`, `""`, empty arrays and objects with no own
  properties. Everything else is truthy. `and`, `or` and `not` give `true` or `false`.
- `==` and `!=` are JavaScript `===` and `!==`. `<`, `<=`, `>`, `>=` need two numbers or two
  strings (string order by UTF-16 code units), else `RenderError`.
- `a in b`: an element of array b (`===`), a substring of string b, or an own property name of
  object b. Any other b is a `RenderError`.

## Output

`{{ }}` turns its value into text: strings as they are, numbers with `String()`, booleans as
`true` or `false`, `null` and `undefined` as nothing, arrays as their elements turned into
text this way and joined with `,`. An object that is not an array is a `RenderError`. With
`autoescape` on, the text is then escaped: `&` `<` `>` `"` `'` become `&amp;` `&lt;` `&gt;`
`&quot;` `&#39;`. Template text outside tags is never escaped.

In `strict` mode, outputting `undefined` and looping over `undefined` are `RenderError`s.
Otherwise they output nothing and run the `else` part. Looping over `null` runs the `else`
part. One loop variable iterates an array. Two (`k, v`) iterate an object's own properties in
insertion order. Anything else (strings, numbers, two variables over an array) is a
`RenderError`. `loop` holds `index` (from 1), `index0`, `first`, `last` and `length` for the
innermost loop.

An include renders the template with the same variables, including loop variables. An include
of a template that is already being rendered (directly or through other includes) is a
`RenderError`, as is an unknown name.

## Filters

Built-ins: `upper`, `lower` and `trim` work on the text of the value as `{{ }}` would produce
it. `length` gives the length of a string or array or the number of own properties of an
object, and 0 for `null` and `undefined`. `default(x)` gives x when the value is `undefined` or
`null`. `join(sep = ",")` joins the texts of an array's elements. `first` and `last` give the
first and last element of an array or character of a string (`undefined` when empty).
`truncate(n)` keeps a string of at most n characters and otherwise gives its first n
characters followed by `...` (n is an integer of at least 0). `replace(a, b)` replaces every
occurrence of a. Both need a string value. `round(d = 0)` needs a number and gives
`Math.round(value * 10 ** d) / 10 ** d`. `sort` gives a sorted copy of an array of numbers
(numeric order) or of strings (UTF-16 code unit order). `raw` marks its value as safe, so `{{ }}` does not
escape it. A filter applied after `raw` receives the plain value and gives an ordinary value
again.

A filter given the wrong kind of value or arguments, and an unknown filter name, are
`RenderError`s when the filter is applied. Custom filters receive the value and the
arguments, and their return value is used as is.

## Command line

`node src/cli.ts TEMPLATE DATA [--strict] [--no-escape]` renders the template file TEMPLATE
with the JSON object in file DATA. Include names are file paths relative to the directory of
TEMPLATE. The rendered text is written to stdout exactly, with nothing added, and the exit
status is 0. On failure nothing is written to stdout. A `TemplateSyntaxError` prints `error: line N: <message>` and any other failure
(other template errors, a missing file, bad JSON) prints `error: <message>`, each as one line
on stderr, with exit status 1. Wrong arguments print a usage message on stderr and exit 2.

## Constraints

Node 26, standard library only, no build step. The project will be checked by running
`node --test tests_hidden/` from the project root, so do not put files in `tests_hidden/`. Put
your own tests elsewhere, for example `test/`, as `*.test.ts` files that use `node:test` and
`node:assert/strict`.
