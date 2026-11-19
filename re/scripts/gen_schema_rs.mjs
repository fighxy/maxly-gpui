// Generate crates/max-proto/src/schema.rs from re/out/schemas.tsv (ExtractSchemas.java).
// One serde struct per C++ struct, modules mirror the C++ namespaces, field order and msgpack
// keys exactly as the official client registers them. Every struct takes defaults for missing
// keys and ignores unknown ones, so a server change does not break decoding.
// Usage: node gen_schema_rs.mjs <schemas.tsv> <out.rs>
import { readFileSync, writeFileSync } from "node:fs";

const [tsvPath, outPath] = process.argv.slice(2);

const KEYWORDS = new Set(("as break const continue crate else enum extern false fn for if impl in let loop match mod move mut " +
  "pub ref return self Self static struct super trait true type unsafe use where while async await dyn abstract become box do " +
  "final macro override priv typeof unsized virtual yield try gen").split(" "));

// ---- parse ----------------------------------------------------------------------------------
const structs = new Map(); // full C++ name -> Map(key -> {cpp, flags})
for (const line of readFileSync(tsvPath, "utf8").split(/\r?\n/)) {
  if (!line) continue;
  const [name, , key, type, flag] = line.split("\t");
  const s = structs.get(name) ?? new Map();
  if (!s.has(key)) s.set(key, { cpp: type, flags: new Set() });
  if (flag) s.get(key).flags.add(flag);
  structs.set(name, s);
}

function splitArgs(s) {
  const out = [];
  let depth = 0;
  let cur = "";
  for (const ch of s) {
    if (ch === "<") depth++;
    if (ch === ">") depth--;
    if (ch === "," && depth === 0) {
      out.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

// ---- names ----------------------------------------------------------------------------------
const snake = (s) =>
  s.replace(/[^A-Za-z0-9]+/g, "_")
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .toLowerCase()
    .replace(/^_+|_+$/g, "")
    .replace(/^(\d)/, "_$1");
const modIdent = (s) => {
  const id = snake(s);
  return KEYWORDS.has(id) ? `r#${id}` : id;
};
const typeIdent = (s) => (KEYWORDS.has(s) ? `${s}_` : s.replace(/[^A-Za-z0-9_]/g, "_"));

function rustPath(cppName) {
  const parts = cppName.split("::");
  const root = parts[0] === "P" ? "packets" : "types";
  const mods = [root, ...parts.slice(1, -1).map(modIdent)];
  return { mods, name: typeIdent(parts[parts.length - 1]) };
}
const absPath = (cppName) => {
  const { mods, name } = rustPath(cppName);
  return `crate::schema::${mods.join("::")}::${name}`;
};

// ---- types ----------------------------------------------------------------------------------
const VALUE = "crate::schema::Any";
const LEAF = {
  __int64: "i64", "unsigned __int64": "u64", int: "i32", "unsigned int": "u32", short: "i16",
  "unsigned short": "u16", signed_char: "i8", char: "i8", "unsigned char": "u8", bool: "bool",
  double: "f64", float: "f32", string: "::std::string::String",
};

// Returns {rs, target (direct struct ref or null), ord (usable as a map key)}.
function rustType(t) {
  t = t.trim().replace(/^(class|struct) /, "").replace(/string_const_/g, "string");
  if (LEAF[t]) return { rs: LEAF[t], target: null, ord: !["f64", "f32"].includes(LEAF[t]) };
  if (structs.has(t)) return { rs: absPath(t), target: t, ord: false };
  const m = t.match(/^([A-Za-z_:]+)<(.*)>$/);
  if (!m) return { rs: VALUE, target: null, ord: false };
  const [, outer, inner] = m;
  const args = splitArgs(inner);
  switch (outer) {
    case "std::optional": {
      const i = rustType(args[0]);
      return { rs: `::std::option::Option<${i.rs}>`, target: i.target, ord: false, optional: true, inner: i };
    }
    case "std::shared_ptr":
    case "std::unique_ptr": {
      const i = rustType(args[0]);
      return { ...i, shared: true };
    }
    case "std::vector":
      if (args[0].trim() === "std::byte") return { rs: "::serde_bytes::ByteBuf", target: null, ord: false };
      return { rs: `::std::vec::Vec<${rustType(args[0]).rs}>`, target: null, ord: false };
    case "std::set":
    case "std::unordered_set": {
      const k = rustType(args[0]);
      return k.ord ? { rs: `::std::collections::BTreeSet<${k.rs}>`, target: null, ord: false } : { rs: VALUE, target: null, ord: false };
    }
    case "std::map":
    case "std::unordered_map": {
      const k = rustType(args[0]);
      const v = rustType(args[1]);
      return k.ord ? { rs: `::std::collections::BTreeMap<${k.rs}, ${v.rs}>`, target: null, ord: false } : { rs: VALUE, target: null, ord: false };
    }
    default:
      return { rs: VALUE, target: null, ord: false };
  }
}

// ---- recursion: box direct/optional edges inside a cycle -----------------------------------
const edges = new Map();
for (const [name, fields] of structs) {
  edges.set(name, new Set([...fields.values()].map((f) => rustType(f.cpp).target).filter(Boolean)));
}
function reaches(from, to, seen = new Set()) {
  if (from === to) return true;
  if (seen.has(from)) return false;
  seen.add(from);
  for (const n of edges.get(from) ?? []) if (reaches(n, to, seen)) return true;
  return false;
}

// ---- module tree ----------------------------------------------------------------------------
const tree = { mods: new Map(), items: [] };
for (const name of [...structs.keys()].sort()) {
  const { mods, name: ident } = rustPath(name);
  let node = tree;
  for (const m of mods) {
    if (!node.mods.has(m)) node.mods.set(m, { mods: new Map(), items: [] });
    node = node.mods.get(m);
  }
  node.items.push({ cpp: name, ident });
}

function renderStruct(cpp, ident, ind) {
  const fields = structs.get(cpp);
  const used = new Set();
  let out = `${ind}/// C++ \`${cpp.replace(/^P::/, "Api::OneMe::Packets::").replace(/^T::/, "Api::OneMe::Types::")}\`.\n`;
  out += `${ind}#[derive(Debug, Clone, PartialEq, Default, ::serde::Serialize, ::serde::Deserialize)]\n`;
  out += `${ind}#[serde(default)]\n${ind}pub struct ${ident} {\n`;
  for (const [key, f] of fields) {
    let fid = snake(key) || "field";
    if (KEYWORDS.has(fid)) fid = `${fid}_`;
    while (used.has(fid)) fid += "_";
    used.add(fid);
    const t = rustType(f.cpp);
    let rs = t.rs;
    const recursive = t.target && reaches(t.target, cpp);
    if (recursive) {
      rs = t.optional
        ? `::std::option::Option<::std::boxed::Box<${t.inner.rs}>>`
        : `::std::boxed::Box<${t.rs}>`;
    }
    const flags = [...f.flags].join(",");
    out += `${ind}    /// Key \`${key}\`${flags ? `, member flag ${flags}` : ""}; C++ \`${f.cpp.replace(/`/g, "")}\`.\n`;
    out += `${ind}    #[serde(rename = ${JSON.stringify(key)}${t.optional ? ', skip_serializing_if = "::std::option::Option::is_none"' : ""})]\n`;
    out += `${ind}    pub ${fid}: ${rs},\n`;
  }
  return out + `${ind}}\n`;
}

function render(node, ind) {
  let out = "";
  for (const item of node.items) out += renderStruct(item.cpp, item.ident, ind) + "\n";
  for (const [m, child] of [...node.mods].sort()) {
    out += `${ind}/// Namespace \`${m.replace(/^r#/, "")}\`.\n${ind}pub mod ${m} {\n${render(child, ind + "    ")}${ind}}\n\n`;
  }
  return out;
}

const header = `// @generated by re/scripts/gen_schema_rs.mjs from re/out/schemas.tsv
// (core.dll of MAX desktop 26.41.0.83760, ExtractSchemas.java). Do not edit by hand: regenerate.
//
// Every struct the official client (de)serializes with SerializableMember, with its msgpack keys
// in the client's order. Missing keys take defaults and unknown keys are ignored when decoding.
// Types without an extracted layout (polymorphic attachments, a few enums) are rmpv::Value.

#![allow(clippy::all, clippy::pedantic, non_snake_case, non_camel_case_types)]

/// Any msgpack value, for members without an extracted layout. \`Nil\` by default.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(transparent)]
pub struct Any(pub ::rmpv::Value);

impl Default for Any {
    fn default() -> Any {
        Any(::rmpv::Value::Nil)
    }
}

`;
writeFileSync(outPath, header + render(tree, ""));
console.log(`${structs.size} structs written to ${outPath}`);
