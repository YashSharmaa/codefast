#!/usr/bin/env python3
"""Checks every packaging file in this repository. Standard library only, no network by default.

    python3 tests/packaging/manifests/validate.py [--schemas DIR] [--fetch] [--binary PATH]

What it checks:
  * every manifest parses, names the plugin `codefast` and carries the version of Cargo.toml;
  * plugin.json and mcp.json follow Agent Plugins 1.0.0 (the rules of the specification, plus
    the published JSON Schemas when they are available);
  * the Claude Code, Codex, Gemini CLI, MCP Registry and npm files agree with each other and
    point at files that exist;
  * the hooks files register exactly the three `codefast hook` commands;
  * the skill keeps to the six portable frontmatter keys;
  * the launcher and install.sh pass `sh -n` (and `dash -n`, shellcheck when installed), agree
    on the release asset names, and refuse to download while the placeholder is in place;
  * docs/INSTALL.md lists every file that contains the OWNER/codefast placeholder.

--schemas DIR   directory holding plugin.schema.json, mcp.schema.json (agent-plugins.org,
                schemas/1.0.0) and server.schema.json (static.modelcontextprotocol.io,
                schemas/2025-12-11). Schemas that are missing are skipped, and said so.
--fetch         download the schemas that are missing into DIR first.
--binary PATH   also check a built codefast: its version, and that the MCP tools named in
                GEMINI.md exist.
--write-placeholder-table
                rewrite the table in docs/INSTALL.md that lists the files containing the
                OWNER/codefast placeholder, instead of only checking it.

Exit status 0 when nothing failed.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
NAME = "codefast"
PLACEHOLDER = "OWNER"
SCHEMA_URLS = {
    "plugin.schema.json": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
    "mcp.schema.json": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
    "server.schema.json": "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json",
}
TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-musl", "x86_64-unknown-linux-musl"]
SKILL_KEYS = {"name", "description", "license", "compatibility", "metadata", "allowed-tools"}

failures = []
notes = []
passed = 0


def ok(message):
    global passed
    passed += 1
    if VERBOSE:
        print("ok    " + message)


def fail(message):
    failures.append(message)
    print("FAIL  " + message)


def check(condition, message, detail=""):
    if condition:
        ok(message)
    else:
        fail(message + (": " + detail if detail else ""))
    return bool(condition)


def path(*parts):
    return os.path.join(ROOT, *parts)


def read(rel):
    with open(path(rel), encoding="utf-8") as f:
        return f.read()


def load(rel):
    try:
        return json.loads(read(rel))
    except FileNotFoundError:
        fail(f"{rel} exists")
    except ValueError as e:
        fail(f"{rel} is valid JSON: {e}")
    return None


# ----- a small JSON Schema validator: the keywords the three schemas use -----


def schema_errors(schema, value, root=None, where="$"):
    root = schema if root is None else root
    if "$ref" in schema:
        target = root
        for part in schema["$ref"].lstrip("#/").split("/"):
            target = target[part]
        merged = {k: v for k, v in schema.items() if k != "$ref"}
        return schema_errors(target, value, root, where) + (schema_errors(merged, value, root, where) if merged else [])
    errors = []
    kinds = {"object": dict, "array": list, "string": str, "boolean": bool, "integer": int, "number": (int, float), "null": type(None)}
    if "type" in schema:
        wanted = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(isinstance(value, kinds[t]) and not (t in ("integer", "number") and isinstance(value, bool)) for t in wanted):
            return [f"{where}: expected {'/'.join(wanted)}"]
    if "const" in schema and value != schema["const"]:
        errors.append(f"{where}: must be {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{where}: must be one of {schema['enum']}")
    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            errors.append(f"{where}: shorter than {schema['minLength']}")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            errors.append(f"{where}: longer than {schema['maxLength']} ({len(value)})")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            errors.append(f"{where}: does not match {schema['pattern']}")
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                errors.append(f"{where}: missing {key}")
        props = schema.get("properties", {})
        extra = schema.get("additionalProperties", True)
        for key, item in value.items():
            if "propertyNames" in schema:
                errors += schema_errors(schema["propertyNames"], key, root, f"{where}.{key}(name)")
            if key in props:
                errors += schema_errors(props[key], item, root, f"{where}.{key}")
            elif extra is False:
                errors.append(f"{where}: unknown field {key}")
            elif isinstance(extra, dict):
                errors += schema_errors(extra, item, root, f"{where}.{key}")
    if isinstance(value, list) and isinstance(schema.get("items"), dict):
        for i, item in enumerate(value):
            errors += schema_errors(schema["items"], item, root, f"{where}[{i}]")
    for sub in schema.get("allOf", []):
        errors += schema_errors(sub, value, root, where)
    if "anyOf" in schema and not any(not schema_errors(sub, value, root, where) for sub in schema["anyOf"]):
        errors.append(f"{where}: matches none of anyOf")
    if "oneOf" in schema:
        matches = sum(1 for sub in schema["oneOf"] if not schema_errors(sub, value, root, where))
        if matches != 1:
            errors.append(f"{where}: matches {matches} of oneOf, expected exactly 1")
    if "not" in schema and not schema_errors(schema["not"], value, root, where):
        errors.append(f"{where}: must not match {schema['not']}")
    return errors


def against_schema(schemas, schema_name, rel, value):
    schema = schemas.get(schema_name)
    if schema is None:
        notes.append(f"{rel}: not checked against {SCHEMA_URLS[schema_name]} (schema file not available; pass --schemas DIR, with --fetch to download)")
        return
    errors = schema_errors(schema, value)
    check(not errors, f"{rel} conforms to {SCHEMA_URLS[schema_name]}", "; ".join(errors[:5]))


def load_schemas(directory, fetch):
    schemas = {}
    if not directory:
        return schemas
    os.makedirs(directory, exist_ok=True)
    for name, url in SCHEMA_URLS.items():
        target = os.path.join(directory, name)
        if not os.path.exists(target) and fetch:
            try:
                request = urllib.request.Request(url, headers={"User-Agent": "schema-check/1.0"})
                with urllib.request.urlopen(request, timeout=30) as response, open(target, "wb") as out:
                    out.write(response.read())
            except OSError as e:
                notes.append(f"could not download {url}: {e}")
        if os.path.exists(target):
            with open(target, encoding="utf-8") as f:
                schemas[name] = json.load(f)
    return schemas


# ----- the checks -----


def cargo_version():
    in_package = False
    for line in read("Cargo.toml").splitlines():
        if line.strip().startswith("["):
            in_package = line.strip() == "[package]"
        match = re.match(r'\s*version\s*=\s*"([^"]+)"', line)
        if in_package and match:
            return match.group(1)
    fail("Cargo.toml has a [package] version")
    return None


def check_agent_plugins(schemas, version):
    plugin = load("plugin.json")
    if plugin is not None:
        allowed = {"$schema", "name", "version", "description", "author", "homepage", "repository", "license", "keywords", "extensions"}
        check(set(plugin) <= allowed, "plugin.json has only Agent Plugins top-level fields", str(sorted(set(plugin) - allowed)))
        check(plugin.get("$schema") == SCHEMA_URLS["plugin.schema.json"], "plugin.json declares the 1.0.0 schema")
        check(re.fullmatch(r"(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?", plugin.get("name", "")) and len(plugin["name"]) <= 64, "plugin.json name is a valid Agent Plugins name")
        check(plugin.get("name") == NAME, f"plugin.json name is {NAME}")
        check(plugin.get("version") == version, f"plugin.json version is {version}", str(plugin.get("version")))
        check(set(plugin.get("author", {})) <= {"name", "email", "url"}, "plugin.json author has only name, email, url")
        extensions = plugin.get("extensions", {})
        check(isinstance(extensions, dict) and all(isinstance(v, dict) for v in extensions.values()), "plugin.json extensions are objects keyed by namespace")
        openai = extensions.get("com.openai", {})
        check("hooks" not in openai, "plugin.json declares no Codex hooks (Codex ignores them for Agent Plugins; hooks/codex.json is the config-layer registration)")
        interface = openai.get("interface", {})
        if interface:
            check(len(interface.get("displayName", "")) <= 30 and interface.get("displayName"), "com.openai displayName is 1-30 characters")
            check(len(interface.get("shortDescription", "")) <= 30 and interface.get("shortDescription"), "com.openai shortDescription is 1-30 characters", str(len(interface.get("shortDescription", ""))))
            check(len(interface.get("longDescription", "")) <= 4000, "com.openai longDescription is at most 4000 characters")
            prompts = interface.get("defaultPrompt", [])
            prompts = [prompts] if isinstance(prompts, str) else prompts
            check(len(prompts) <= 3 and all(len(p) <= 128 for p in prompts), "com.openai defaultPrompt: at most 3 prompts of at most 128 characters")
            # Codex shows these as labels. OpenAI's own plugin manifests use the three below and
            # nothing else; anything written here in free text belongs in longDescription.
            labels = {"Interactive", "Read", "Write"}
            check(set(interface.get("capabilities", [])) <= labels, "com.openai capabilities are labels Codex knows (Interactive, Read, Write)", str(interface.get("capabilities")))
            check("Write" not in interface.get("capabilities", []), "com.openai capabilities do not claim Write (every codefast tool is read-only)")
        against_schema(schemas, "plugin.schema.json", "plugin.json", plugin)

    mcp = load("mcp.json")
    if mcp is not None:
        check(set(mcp) == {"$schema", "mcpServers"}, "mcp.json has exactly $schema and mcpServers")
        check(mcp.get("$schema") == SCHEMA_URLS["mcp.schema.json"], "mcp.json declares the 1.0.0 schema (same version as plugin.json)")
        servers = mcp.get("mcpServers", {})
        check(list(servers) == [NAME], f"mcp.json declares one server named {NAME}", str(list(servers)))
        for name, server in servers.items():
            check(set(server) <= {"type", "command", "args", "env", "cwd"}, f"mcp.json {name}: only stdio fields")
            check(server.get("type") == "stdio", f"mcp.json {name}: type is stdio")
            command = server.get("command", "")
            check(command.startswith("./") and " " not in command and "${" not in command, f"mcp.json {name}: command is one plugin-relative token without placeholders", command)
            target = path(command)
            check(os.path.isfile(target) and os.access(target, os.X_OK), f"mcp.json {name}: {command} exists and is executable")
            check(all(isinstance(a, str) for a in server.get("args", [])) and "mcp" in server.get("args", []), f"mcp.json {name}: args are strings and start the MCP server")
            check(not {"PLUGIN_ROOT", "PLUGIN_DATA"} & set(server.get("env", {})), f"mcp.json {name}: env does not set the reserved variables")
            text = json.dumps(server)
            check("${" not in text, f"mcp.json {name}: no placeholders at all (Cursor does not expand ${{PLUGIN_ROOT}})")
        against_schema(schemas, "mcp.schema.json", "mcp.json", mcp)


def check_claude(version):
    manifest = load(".claude-plugin/plugin.json")
    if manifest is not None:
        check(manifest.get("name") == NAME, f".claude-plugin/plugin.json name is {NAME}")
        check(manifest.get("version") == version, f".claude-plugin/plugin.json version is {version}", str(manifest.get("version")))
        check(bool(manifest.get("description")) and bool(manifest.get("author", {}).get("name")), ".claude-plugin/plugin.json has a description and an author name")
        hooks = manifest.get("hooks")
        check(isinstance(hooks, str) and hooks.startswith("./") and os.path.isfile(path(hooks)), ".claude-plugin/plugin.json hooks is a ./ path to an existing file", str(hooks))
        servers = manifest.get("mcpServers", {})
        check(isinstance(servers, dict) and list(servers) == [NAME], f".claude-plugin/plugin.json declares one inline MCP server named {NAME}")
        for name, server in servers.items() if isinstance(servers, dict) else []:
            command = server.get("command", "")
            check(command.startswith("${CLAUDE_PLUGIN_ROOT}/"), f".claude-plugin/plugin.json {name}: command starts at ${{CLAUDE_PLUGIN_ROOT}}", command)
            rel = command.replace("${CLAUDE_PLUGIN_ROOT}/", "")
            check(os.path.isfile(path(rel)) and os.access(path(rel), os.X_OK), f".claude-plugin/plugin.json {name}: {rel} exists and is executable")
            check(server.get("args") == ["mcp"], f".claude-plugin/plugin.json {name}: args are [mcp]")
        for key in ("skills", "commands", "agents", "outputStyles", "lspServers"):
            value = manifest.get(key)
            for item in [value] if isinstance(value, str) else value if isinstance(value, list) else []:
                if isinstance(item, str):
                    check(item.startswith("./") or item == ".", f".claude-plugin/plugin.json {key} path starts with ./", item)

    market = load(".claude-plugin/marketplace.json")
    if market is not None:
        check(market.get("name") == NAME, f".claude-plugin/marketplace.json name is {NAME}")
        check(bool(market.get("owner", {}).get("name")), ".claude-plugin/marketplace.json has an owner name")
        entries = market.get("plugins", [])
        check(len(entries) == 1 and entries[0].get("name") == NAME, f".claude-plugin/marketplace.json lists the plugin {NAME} (same name as the manifest)")
        check(entries and entries[0].get("source") == "./", ".claude-plugin/marketplace.json source is ./ (the repository root is the plugin)")
        check(not entries or "version" not in entries[0], ".claude-plugin/marketplace.json entry has no version (plugin.json is the one source)")

    check(not os.path.exists(path("CLAUDE.md")), "there is no CLAUDE.md at the plugin root (Claude Code does not load it and warns)")
    check(not os.path.exists(path(".mcp.json")), "there is no .mcp.json at the repository root (it would be project-scope MCP config for anyone who opens the repository)")
    check(
        not os.path.exists(path("hooks", "hooks.json")),
        "there is no hooks/hooks.json (Claude Code, Codex, Gemini CLI, Goose and Cursor-format plugins all read that path, with different schemas)",
    )
    for folder in ("commands", "agents", "output-styles", "workflows", "themes", "monitors"):
        check(not os.path.isdir(path(folder)), f"there is no {folder}/ at the repository root (Claude Code would load it as plugin content)")
    check(not os.path.exists(path("settings.json")), "there is no settings.json at the repository root (Claude Code would apply it as plugin settings)")


HOOK_COMMANDS = {
    "SessionStart": "session-start",
    "UserPromptSubmit": "user-prompt-submit",
    "PreToolUse": "pre-tool-use",
    "PostToolUse": "post-tool-use",
    "PostToolUseFailure": "post-tool-use",
}


def check_hooks_file(rel, events, plugin_relative):
    data = load(rel)
    if data is None:
        return
    check(set(data) <= {"description", "hooks"} and isinstance(data.get("hooks"), dict), f"{rel} wraps the event map in a top-level hooks object")
    hooks = data.get("hooks", {})
    check(set(hooks) == set(events), f"{rel} registers exactly {sorted(events)}", str(sorted(hooks)))
    for event, groups in hooks.items():
        check(isinstance(groups, list) and groups, f"{rel} {event}: a non-empty list of matcher groups")
        for group in groups if isinstance(groups, list) else []:
            if event in ("SessionStart", "UserPromptSubmit"):
                check("matcher" not in group, f"{rel} {event}: no matcher (every session source, every prompt)")
            else:
                matcher = group.get("matcher", "")
                check(bool(matcher) and re.fullmatch(r"[A-Za-z0-9_|]+", matcher), f"{rel} {event}: matcher is a plain list of tool names (exact match in Claude Code, regex in Codex)", matcher)
                check("Bash" in matcher.split("|"), f"{rel} {event}: matcher includes Bash (where searches run in Claude Code on macOS and Linux, and always in Codex)")
            for handler in group.get("hooks", []):
                check(set(handler) <= {"type", "command", "timeout"}, f"{rel} {event}: handler uses only type, command, timeout", str(sorted(handler)))
                check(handler.get("type") == "command", f"{rel} {event}: handler type is command")
                command = handler.get("command", "")
                wanted = f"hook {HOOK_COMMANDS.get(event, '?')}"
                check(wanted in command, f"{rel} {event}: runs `codefast {wanted}`", command)
                if plugin_relative:
                    check(command.startswith('"${CLAUDE_PLUGIN_ROOT}/bin/codefast" '), f"{rel} {event}: launcher path is quoted and plugin-relative", command)
                else:
                    check(command.startswith("codefast hook ") and "${" not in command, f"{rel} {event}: uses codefast from PATH", command)
                timeout = handler.get("timeout")
                check(isinstance(timeout, int) and 1 <= timeout <= 30, f"{rel} {event}: timeout is a few seconds", str(timeout))


def check_codex(version):
    market = load(".agents/plugins/marketplace.json")
    if market is None:
        return
    check(market.get("name") == NAME, f".agents/plugins/marketplace.json name is {NAME}")
    check(bool(market.get("interface", {}).get("displayName")), ".agents/plugins/marketplace.json has interface.displayName")
    entries = market.get("plugins", [])
    check(len(entries) == 1 and entries[0].get("name") == NAME, f".agents/plugins/marketplace.json lists the plugin {NAME}")
    for entry in entries:
        source = entry.get("source")
        local = source == "./" or (isinstance(source, dict) and source.get("source") == "local" and source.get("path") == "./")
        check(local, ".agents/plugins/marketplace.json source is the marketplace root itself (./)", json.dumps(source))
        policy = entry.get("policy", {})
        check(policy.get("installation") in {"AVAILABLE", "INSTALLED_BY_DEFAULT", "NOT_AVAILABLE"}, ".agents/plugins/marketplace.json policy.installation is set")
        check(bool(policy.get("authentication")), ".agents/plugins/marketplace.json policy.authentication is set")
        check(bool(entry.get("category")), ".agents/plugins/marketplace.json category is set")
    check(not os.path.exists(path(".codex-plugin")), "there is no .codex-plugin/ overlay (plugin.json carries extensions.com.openai, which replaces it)")
    yaml_rel = f"skills/{NAME}/agents/openai.yaml"
    if os.path.exists(path(yaml_rel)):
        text = read(yaml_rel)
        tops = re.findall(r"^([a-z_]+):", text, re.M)
        check(set(tops) <= {"interface", "policy", "dependencies"}, f"{yaml_rel} has only interface, policy, dependencies", str(tops))
        check("dependencies" not in tops, f"{yaml_rel} declares no MCP dependency (the skill runs the command line tool)")
        check(not re.search(r"\t", text), f"{yaml_rel} is indented with spaces")


def check_gemini(version):
    ext = load("gemini-extension.json")
    if ext is None:
        return
    check(ext.get("name") == NAME, f"gemini-extension.json name is {NAME}")
    check(ext.get("version") == version, f"gemini-extension.json version is {version}", str(ext.get("version")))
    known = {"name", "version", "description", "mcpServers", "contextFileName", "excludeTools", "settings", "themes", "plan", "migratedTo"}
    check(set(ext) <= known, "gemini-extension.json has only documented fields", str(sorted(set(ext) - known)))
    context = ext.get("contextFileName")
    check(isinstance(context, str) and os.path.isfile(path(context)), "gemini-extension.json contextFileName names an existing file", str(context))
    if isinstance(context, str) and os.path.isfile(path(context)):
        lines = read(context).splitlines()
        check(len(lines) <= 40, f"{context} is at most 40 lines", str(len(lines)))
    servers = ext.get("mcpServers", {})
    check(list(servers) == [NAME], f"gemini-extension.json declares one MCP server named {NAME}")
    for name, server in servers.items():
        command = server.get("command", "")
        check(command == "${extensionPath}${/}bin${/}codefast", f"gemini-extension.json {name}: command is the launcher inside the extension", command)
        args = server.get("args", [])
        check(args[:1] == ["mcp"] and "--root" in args and "${workspacePath}" in args, f"gemini-extension.json {name}: passes the workspace as --root", str(args))
        check("trust" not in server, f"gemini-extension.json {name}: does not set trust (extensions may not)")
    check(not os.path.isdir(path("policies")), "there is no policies/ at the repository root (Gemini CLI would load it as extension policy)")


def check_registry(schemas, version):
    server = load("server.json")
    if server is None:
        return None
    check(server.get("$schema") == SCHEMA_URLS["server.schema.json"], "server.json declares the 2025-12-11 schema")
    name = server.get("name", "")
    check(re.fullmatch(r"[a-zA-Z0-9.-]+/[a-zA-Z0-9._-]+", name) and name.endswith("/" + NAME), "server.json name is NAMESPACE/codefast", name)
    check(1 <= len(server.get("description", "")) <= 100, "server.json description is 1-100 characters", str(len(server.get("description", ""))))
    check(server.get("version") == version, f"server.json version is {version}", str(server.get("version")))
    packages = server.get("packages", [])
    check(len(packages) >= 1, "server.json lists at least one package")
    for package in packages:
        kind = package.get("registryType")
        check(kind in {"npm", "cargo", "mcpb", "oci", "pypi", "nuget"}, "server.json package registryType is a registry the MCP Registry supports", str(kind))
        check(package.get("transport") == {"type": "stdio"}, f"server.json {kind} package: stdio transport")
        check(package.get("version", version) == version, f"server.json {kind} package version is {version}", str(package.get("version")))
        arguments = [a.get("value") for a in package.get("packageArguments", [])]
        check(arguments == ["mcp"], f"server.json {kind} package: started with the positional argument mcp", str(arguments))
        if kind == "mcpb":
            check(re.fullmatch(r"[a-f0-9]{64}", package.get("fileSha256", "")) is not None, "server.json mcpb package has a fileSha256")
            check("mcp" in package.get("identifier", ""), "server.json mcpb URL contains the string mcp")
    against_schema(schemas, "server.schema.json", "server.json", server)
    return server


def check_npm(version, server):
    wrapper = load("npm/codefast/package.json")
    if wrapper is None:
        return
    scope = wrapper.get("name", "").split("/")[0]
    check(wrapper.get("name", "").endswith("/" + NAME) and scope.startswith("@"), "npm wrapper is a scoped package named codefast", wrapper.get("name", ""))
    check(wrapper.get("version") == version, f"npm wrapper version is {version}", str(wrapper.get("version")))
    check("scripts" not in wrapper, "npm wrapper has no lifecycle scripts (harnesses fetch packages without running them)")
    bin_rel = wrapper.get("bin", {}).get(NAME, "")
    check(bool(bin_rel) and os.path.isfile(path("npm/codefast", bin_rel)), "npm wrapper bin.codefast names an existing file", bin_rel)
    check(bin_rel in wrapper.get("files", []), "npm wrapper publishes its bin file")
    if server is not None:
        npm_packages = [p for p in server.get("packages", []) if p.get("registryType") == "npm"]
        check(all(p.get("identifier") == wrapper.get("name") for p in npm_packages), "server.json npm identifier is the npm wrapper's name")
        check(not npm_packages or wrapper.get("mcpName") == server.get("name"), "npm wrapper mcpName equals the server.json name (the registry's ownership check)", str(wrapper.get("mcpName")))
    optional = wrapper.get("optionalDependencies", {})
    platform_root = path("npm/platforms")
    folders = sorted(d for d in os.listdir(platform_root) if os.path.isdir(os.path.join(platform_root, d))) if os.path.isdir(platform_root) else []
    check(sorted(optional) == [f"{scope}/{NAME}-{d}" for d in folders], "npm wrapper optionalDependencies match npm/platforms/*", f"{sorted(optional)} vs {folders}")
    check(all(v == version for v in optional.values()), f"npm wrapper pins every platform package to {version}", str(optional))
    for folder in folders:
        rel = f"npm/platforms/{folder}/package.json"
        package = load(rel)
        if package is None:
            continue
        osname, cpu = folder.split("-", 1)
        check(package.get("name") == f"{scope}/{NAME}-{folder}", f"{rel} name", str(package.get("name")))
        check(package.get("version") == version, f"{rel} version is {version}", str(package.get("version")))
        check(package.get("os") == [osname] and package.get("cpu") == [cpu], f"{rel} os and cpu match the folder name")
        exe = "codefast.exe" if osname == "win32" else "codefast"
        check(package.get("files") == [f"bin/{exe}"], f"{rel} publishes bin/{exe}")
        check("scripts" not in package, f"{rel} has no lifecycle scripts")
    shim = read("npm/codefast/" + bin_rel) if bin_rel and os.path.isfile(path("npm/codefast", bin_rel)) else ""
    check("-${process.platform}-${process.arch}" in shim, "npm shim resolves the platform package from process.platform and process.arch")
    check("console.log" not in shim, "npm shim never writes to stdout")
    check("realpathSync(__filename)" in shim, "npm shim recognises CODEFAST_BIN that names the shim itself (it would start itself without end)")
    if shutil.which("node"):
        result = subprocess.run(["node", "--check", path("npm/codefast", bin_rel)], capture_output=True, text=True)
        check(result.returncode == 0, "npm shim passes node --check", result.stderr.strip()[:200])
    else:
        notes.append("node is not installed: npm shim syntax not checked")


def check_skill():
    rel = f"skills/{NAME}/SKILL.md"
    text = read(rel)
    match = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if not check(match is not None, f"{rel} starts with YAML frontmatter"):
        return
    keys = re.findall(r"^([A-Za-z][\w-]*):", match.group(1), re.M)
    check(set(keys) <= SKILL_KEYS, f"{rel} uses only the six portable frontmatter keys", str(sorted(set(keys) - SKILL_KEYS)))
    fields = dict(re.findall(r"^([A-Za-z][\w-]*):\s*(.*)$", match.group(1), re.M))
    check(fields.get("name") == NAME, f"{rel} name equals its directory name")
    check(1 <= len(fields.get("description", "")) <= 1024, f"{rel} description is 1-1024 characters", str(len(fields.get("description", ""))))
    check(len(text.splitlines()) < 500, f"{rel} is under 500 lines")
    others = [d for d in os.listdir(path("skills")) if os.path.isdir(path("skills", d))]
    check(others == [NAME], "skills/ holds exactly one skill, as an immediate child directory", str(others))


def check_scripts(version):
    launcher = read("bin/codefast")
    lines = launcher.splitlines()
    check(lines[0] == "#!/bin/sh" and "codefast-launcher" in lines[1], "bin/codefast: the second line carries the launcher marker")
    check(os.access(path("bin/codefast"), os.X_OK) and os.access(path("install.sh"), os.X_OK), "bin/codefast and install.sh are executable")
    match = re.search(r"^version=(\S+)$", launcher, re.M)
    check(match is not None and match.group(1) == version, f"bin/codefast pins version {version}", match.group(1) if match else "no version= line")
    installer = read("install.sh")
    for name, text in (("bin/codefast", launcher), ("install.sh", installer)):
        check("asset=codefast-$target.tar.gz" in text, f"{name} asks for codefast-<target>.tar.gz")
        check("SHA256SUMS" in text and "checksum mismatch" in text, f"{name} verifies the SHA-256 against SHA256SUMS")
        triples = sorted(set(a + s for a in ("aarch64", "x86_64") for s in re.findall(r"printf '%s(-[a-z-]+)\\n'", text)))
        check(triples == sorted(TARGETS), f"{name} knows the four release targets", str(triples))
        check("sudo " not in re.sub(r"#.*|err \".*|echo \".*", "", text), f"{name} never runs sudo")
        for shell in ("sh", "dash"):
            if shutil.which(shell):
                result = subprocess.run([shell, "-n", path(name)], capture_output=True, text=True)
                check(result.returncode == 0, f"{name} passes {shell} -n", result.stderr.strip()[:200])
        if shutil.which("shellcheck"):
            result = subprocess.run(["shellcheck", "-s", "sh", "-S", "style", path(name)], capture_output=True, text=True)
            check(result.returncode == 0, f"{name} passes shellcheck -s sh", result.stdout.strip()[:400])
        else:
            notes.append(f"shellcheck is not installed: {name} not linted")

    def body(text, function):
        match = re.search(r"^%s\(\) \{\n(.*?)^\}" % function, text, re.S | re.M)
        return match.group(1) if match else None

    for function in ("detect_target", "sum_for", "unpublished"):
        check(body(launcher, function) is not None and body(launcher, function) == body(installer, function), f"bin/codefast and install.sh share the same {function}()")
    # While the repository has no public home its URLs name a stand-in owner, which is somebody
    # else's account: neither script may fetch from it. The test must not contain the stand-in
    # itself, or replacing it everywhere would rewrite the test instead of turning it off.
    for name, text in (("bin/codefast", launcher), ("install.sh", installer)):
        guard = body(text, "unpublished")
        check(guard is not None and PLACEHOLDER not in guard, f"{name}: unpublished() is written without the placeholder itself")
        asks = re.search(r"^\s*if unpublished ", text, re.M)
        fetches = re.search(r"^\s*if ! fetch ", text, re.M)
        check(asks is not None and fetches is not None and asks.start() < fetches.start(), f"{name}: asks unpublished() before its first download")
    check(
        re.search(r"release_base=\$\{CODEFAST_RELEASE_BASE:-(\S+)\}", launcher).group(1) == re.search(r"release_base=\$\{CODEFAST_RELEASE_BASE:-(\S+)\}", installer).group(1),
        "bin/codefast and install.sh use the same default release URL",
    )
    stdout_writes = [line for line in launcher.splitlines() if re.match(r"\s*(echo|printf)\b", line) and ">&2" not in line and ">" not in line and "printf '%s" not in line]
    check(not stdout_writes, "bin/codefast has no bare echo or printf that could reach stdout", str(stdout_writes[:3]))


OWNED = [
    "plugin.json",
    "mcp.json",
    "gemini-extension.json",
    "GEMINI.md",
    "AGENTS.md",
    "server.json",
    "install.sh",
    ".claude-plugin",
    ".agents",
    ".cursor-plugin",
    ".codex-plugin",
    "com.github.copilot",
    "hooks",
    "bin",
    "npm",
    "skills",
    "docs/INSTALL.md",
    "docs/HARNESSES.md",
]


def placeholder_files():
    found = {}
    for entry in OWNED:
        top = path(entry)
        files = [top] if os.path.isfile(top) else [os.path.join(d, f) for d, _, names in os.walk(top) for f in names]
        for file in files:
            try:
                with open(file, encoding="utf-8") as f:
                    text = f.read()
            except (OSError, UnicodeDecodeError):
                continue
            count = text.count(PLACEHOLDER)
            if count:
                found[os.path.relpath(file, ROOT)] = (count, text)
    return found


TABLE_BEGIN = "<!-- placeholder-table:begin -->"
TABLE_END = "<!-- placeholder-table:end -->"


def placeholder_table(found):
    rows = ["| File | Occurrences |", "|---|---|"]
    rows += [f"| `{rel}` | {count} |" for rel, (count, _) in sorted(found.items())]
    return "\n".join(rows)


def check_placeholder(write):
    found = placeholder_files()
    for rel, (count, text) in sorted(found.items()):
        bare = re.findall(r"(?<![@./])\bOWNER\b(?!/codefast)", text)
        if not rel.startswith("docs/"):
            check(not bare, f"{rel}: every OWNER is part of OWNER/codefast", f"{len(bare)} bare occurrence(s)")
    rel = "docs/INSTALL.md"
    if not os.path.exists(path(rel)):
        notes.append(f"{rel} does not exist yet: the placeholder list was not checked")
        return
    text = read(rel)
    if "## Before publishing" not in text:
        check(not found, f"no file contains the placeholder any more (the 'Before publishing' section of {rel} is gone)", str(sorted(found)))
        return
    if not check(TABLE_BEGIN in text and TABLE_END in text, f"{rel} 'Before publishing' has the placeholder table markers"):
        return
    head, rest = text.split(TABLE_BEGIN, 1)
    current, tail = rest.split(TABLE_END, 1)
    wanted = placeholder_table(found)
    if write and current.strip() != wanted:
        with open(path(rel), "w", encoding="utf-8") as f:
            f.write(head + TABLE_BEGIN + "\n" + wanted + "\n" + TABLE_END + tail)
        print(f"wrote the placeholder table in {rel}")
        current = wanted
    check(current.strip() == wanted, f"{rel} 'Before publishing' lists every packaging file that contains the placeholder, with the right counts", "run validate.py --write-placeholder-table")


def check_binary(binary, version):
    result = subprocess.run([binary, "--version"], capture_output=True, text=True)
    check(result.stdout.strip() == f"codefast {version}", f"the binary reports version {version}", result.stdout.strip())
    request = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "validate", "version": "0"}}}
    listing = {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}
    payload = json.dumps(request) + "\n" + json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n" + json.dumps(listing) + "\n"
    result = subprocess.run([binary, "mcp", "--root", ROOT], input=payload, capture_output=True, text=True, timeout=60)
    tools = []
    for line in result.stdout.splitlines():
        try:
            reply = json.loads(line)
        except ValueError:
            fail("the MCP server writes only JSON lines to stdout")
            continue
        if reply.get("id") == 2:
            tools = [t["name"] for t in reply.get("result", {}).get("tools", [])]
    check(bool(tools), "the binary answers tools/list", result.stderr.strip()[:200])
    gemini = read("GEMINI.md")
    named = re.search(r"`codefast` MCP server \(([^)]*)\)", gemini)
    if check(named is not None, "GEMINI.md names the MCP tools"):
        mentioned = re.findall(r"`([a-z_]+)`", named.group(1))
        check(set(mentioned) == set(tools), "GEMINI.md names exactly the tools the MCP server offers", f"GEMINI.md {mentioned}, server {tools}")
    description = load("plugin.json")["extensions"]["com.openai"]["interface"]["longDescription"]
    words = {1: "one", 2: "two", 3: "three", 4: "four", 5: "five", 6: "six", 7: "seven", 8: "eight"}
    check(f"{words.get(len(tools), len(tools))} read-only tools" in description, "plugin.json longDescription states the real number of MCP tools", str(len(tools)))


def main():
    global VERBOSE
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--schemas")
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--binary")
    parser.add_argument("--write-placeholder-table", action="store_true")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()
    VERBOSE = args.verbose
    schemas = load_schemas(args.schemas, args.fetch)

    version = cargo_version()
    check_agent_plugins(schemas, version)
    check_claude(version)
    check_hooks_file("hooks/claude-code.json", {"SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure"}, plugin_relative=True)
    check_hooks_file("hooks/codex.json", {"SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse"}, plugin_relative=False)
    check_codex(version)
    check_gemini(version)
    server = check_registry(schemas, version)
    check_npm(version, server)
    check_skill()
    check_scripts(version)
    check_placeholder(args.write_placeholder_table)
    if args.binary:
        check_binary(args.binary, version)
    else:
        notes.append("no --binary given: version and MCP tool names were not checked against a built codefast")

    for note in notes:
        print("note  " + note)
    print(f"{passed} passed, {len(failures)} failed")
    return 1 if failures else 0


VERBOSE = False
if __name__ == "__main__":
    sys.exit(main())
