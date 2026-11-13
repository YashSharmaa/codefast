"""Print ast-grep inline rules (YAML documents separated by ---) for one query.

usage: python3 -I _astgrep_rules.py def|callers|callees NAME

ast-grep has no index and no cross-language "find symbol" verb, so each query is a set of structural
rules, one or more per language, that `ast-grep scan --inline-rules` applies in a single pass over the
tree. Languages covered: Python, C, C++, Go, Rust, Java, TypeScript, TSX, JavaScript.

  def      nodes that DEFINE NAME (functions, methods, function-like macros, classes/structs/types)
  callers  call sites whose callee name is NAME (the enclosing function is not part of the match)
  callees  call sites lexically inside a function/method named NAME

Matching is purely syntactic and by bare name: no scopes, no types, no imports.
"""
import json
import re
import sys

kind, name = sys.argv[1], sys.argv[2]
N = "^" + re.escape(name) + "$"            # exact identifier
NQ = "(^|::|\\.)" + re.escape(name) + "$"   # last component of a qualified name


def has_name(field="name", rx=N):
    return {"has": {"field": field, "regex": rx}}


# C / C++: the function name sits under declarator -> (pointer_declarator ->)* function_declarator -> declarator
def c_fn_decl(rx):
    return {
        "has": {
            "stopBy": {"kind": "compound_statement"},
            "kind": "function_declarator",
            "has": {"field": "declarator", "regex": rx},
        }
    }


DEFS = {
    "python": [
        {"kind": "function_definition", **has_name()},
        {"kind": "class_definition", **has_name()},
    ],
    "c": [
        {"kind": "function_definition", **c_fn_decl(N)},
        {"kind": "preproc_function_def", **has_name()},
        {"kind": "preproc_def", **has_name()},
        {"kind": "struct_specifier", "all": [has_name(), {"has": {"field": "body", "kind": "field_declaration_list"}}]},
        {"kind": "union_specifier", "all": [has_name(), {"has": {"field": "body", "kind": "field_declaration_list"}}]},
        {"kind": "enum_specifier", "all": [has_name(), {"has": {"field": "body", "kind": "enumerator_list"}}]},
        {"kind": "type_definition", "has": {"field": "declarator", "regex": N}},
    ],
    "cpp": [
        {"kind": "function_definition", **c_fn_decl(NQ)},
        {"kind": "preproc_function_def", **has_name()},
        {"kind": "preproc_def", **has_name()},
        {"kind": "class_specifier", "all": [has_name(), {"has": {"field": "body", "kind": "field_declaration_list"}}]},
        {"kind": "struct_specifier", "all": [has_name(), {"has": {"field": "body", "kind": "field_declaration_list"}}]},
        {"kind": "enum_specifier", "all": [has_name(), {"has": {"field": "body", "kind": "enumerator_list"}}]},
        {"kind": "type_definition", "has": {"field": "declarator", "regex": N}},
        {"kind": "alias_declaration", **has_name()},
    ],
    "go": [
        {"kind": "function_declaration", **has_name()},
        {"kind": "method_declaration", **has_name()},
        {"kind": "type_spec", **has_name()},
    ],
    "rust": [
        {"kind": "function_item", **has_name()},
        {"kind": "function_signature_item", **has_name()},
        {"kind": "struct_item", **has_name()},
        {"kind": "enum_item", **has_name()},
        {"kind": "trait_item", **has_name()},
        {"kind": "type_item", **has_name()},
        {"kind": "macro_definition", **has_name()},
    ],
    "java": [
        {"kind": "method_declaration", **has_name()},
        {"kind": "constructor_declaration", **has_name()},
        {"kind": "class_declaration", **has_name()},
        {"kind": "interface_declaration", **has_name()},
        {"kind": "enum_declaration", **has_name()},
        {"kind": "record_declaration", **has_name()},
    ],
}
_js = [
    {"kind": "function_declaration", **has_name()},
    {"kind": "generator_function_declaration", **has_name()},
    {"kind": "method_definition", **has_name()},
    {"kind": "class_declaration", **has_name()},
    {
        "kind": "variable_declarator",
        "all": [has_name(), {"has": {"field": "value", "any": [{"kind": "arrow_function"}, {"kind": "function_expression"}]}}],
    },
]
_ts = _js + [
    {"kind": "interface_declaration", **has_name()},
    {"kind": "type_alias_declaration", **has_name()},
    {"kind": "enum_declaration", **has_name()},
    {"kind": "abstract_class_declaration", **has_name()},
    {"kind": "method_signature", **has_name()},
]
DEFS["javascript"] = _js
DEFS["typescript"] = _ts
DEFS["tsx"] = _ts


# call-site rules: a call node whose callee names NAME
def callee_is(fn_field, alts):
    return {"has": {"field": fn_field, "any": alts}}


ID = {"regex": N}
CALLS = {
    "python": [
        {"kind": "call", **callee_is("function", [
            {"kind": "identifier", **ID},
            {"kind": "attribute", "has": {"field": "attribute", **ID}},
        ])},
    ],
    "c": [
        {"kind": "call_expression", **callee_is("function", [
            {"kind": "identifier", **ID},
            {"kind": "field_expression", "has": {"field": "field", **ID}},
        ])},
    ],
    "cpp": [
        {"kind": "call_expression", **callee_is("function", [
            {"kind": "identifier", **ID},
            {"kind": "field_expression", "has": {"field": "field", **ID}},
            {"kind": "qualified_identifier", "regex": NQ},
            {"kind": "template_function", "has": {"field": "name", **ID}},
        ])},
    ],
    "go": [
        {"kind": "call_expression", **callee_is("function", [
            {"kind": "identifier", **ID},
            {"kind": "selector_expression", "has": {"field": "field", **ID}},
        ])},
    ],
    "rust": [
        {"kind": "call_expression", **callee_is("function", [
            {"kind": "identifier", **ID},
            {"kind": "scoped_identifier", "has": {"field": "name", **ID}},
            {"kind": "field_expression", "has": {"field": "field", **ID}},
        ])},
        {"kind": "macro_invocation", "has": {"field": "macro", "regex": NQ}},
    ],
    "java": [
        {"kind": "method_invocation", **has_name()},
        {"kind": "object_creation_expression", "has": {"field": "type", "regex": NQ}},
    ],
}
_jscall = [
    {"kind": "call_expression", **callee_is("function", [
        {"kind": "identifier", **ID},
        {"kind": "member_expression", "has": {"field": "property", **ID}},
    ])},
    {"kind": "new_expression", "has": {"field": "constructor", "regex": NQ}},
]
CALLS["javascript"] = CALLS["typescript"] = CALLS["tsx"] = _jscall

# any call node, per language (for callees)
ANY_CALL = {
    "python": ["call"],
    "c": ["call_expression"],
    "cpp": ["call_expression"],
    "go": ["call_expression"],
    "rust": ["call_expression", "macro_invocation"],
    "java": ["method_invocation", "object_creation_expression"],
    "javascript": ["call_expression", "new_expression"],
    "typescript": ["call_expression", "new_expression"],
    "tsx": ["call_expression", "new_expression"],
}
# function-like definitions named NAME (for callees: "inside")
FN_DEFS = {
    "python": [{"kind": "function_definition", **has_name()}],
    "c": [{"kind": "function_definition", **c_fn_decl(N)}],
    "cpp": [{"kind": "function_definition", **c_fn_decl(NQ)}],
    "go": [{"kind": "function_declaration", **has_name()}, {"kind": "method_declaration", **has_name()}],
    "rust": [{"kind": "function_item", **has_name()}],
    "java": [{"kind": "method_declaration", **has_name()}, {"kind": "constructor_declaration", **has_name()}],
    "javascript": [_js[0], _js[1], _js[2], _js[4]],
    "typescript": [_js[0], _js[1], _js[2], _js[4]],
    "tsx": [_js[0], _js[1], _js[2], _js[4]],
}

docs = []
for lang in ("python", "c", "cpp", "go", "rust", "java", "typescript", "tsx", "javascript"):
    if kind == "def":
        rule = {"any": DEFS[lang]}
    elif kind == "callers":
        rule = {"any": CALLS[lang]}
    elif kind == "callees":
        rule = {
            "any": [{"kind": k} for k in ANY_CALL[lang]],
            "inside": {"stopBy": "end", "any": FN_DEFS[lang]},
        }
    else:
        sys.exit("kind must be def, callers or callees")
    docs.append({"id": f"{kind}-{lang}", "language": lang, "severity": "hint", "message": f"{kind} {name}", "rule": rule})

# JSON is valid YAML; one document per rule.
print("\n---\n".join(json.dumps(d) for d in docs))
