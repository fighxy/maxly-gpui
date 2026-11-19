// Build a markdown opcode table from opcodes_desktop.tsv, with PyMax names as hints only.
// Usage: node opcodes_md.mjs <opcodes_desktop.tsv> <pymax protocol/enums.py> <out.md>
import { readFileSync, writeFileSync } from "node:fs";

const [tsvPath, enumsPath, outPath] = process.argv.slice(2);

const pymax = new Map();
const enumsSrc = readFileSync(enumsPath, "utf8");
const opcodeClass = enumsSrc.slice(enumsSrc.indexOf("class Opcode"));
for (const m of opcodeClass.matchAll(/^\s+([A-Z0-9_]+) = (\d+)$/gm)) {
  const code = Number(m[2]);
  if (!pymax.has(code)) pymax.set(code, m[1]);
}

const strip = (t) => t.replace(/::(Parameters|Payload|Response)$/, "");
const rows = new Map();
for (const line of readFileSync(tsvPath, "utf8").split(/\r?\n/)) {
  if (!line) continue;
  const [op, kind, req, resp, flag] = line.split("\t");
  const name = kind === "event" ? strip(resp).replace(/^Events::/, "") : strip(req);
  const r = rows.get(op) ?? { kind: new Set(), names: new Set(), resp: new Set(), flags: new Set() };
  r.kind.add(kind);
  r.names.add(name);
  r.resp.add(strip(resp));
  if (flag) r.flags.add(flag);
  rows.set(op, r);
}

// Request types that are "Events::NoParameters" carry no request name; drop them from names.
const ops = [...rows.keys()].map(Number).sort((a, b) => a - b);
let md = "| Opcode | Kind | Desktop type | Response type | Flag | PyMax name (hint) |\n";
md += "|---:|---|---|---|---|---|\n";
for (const op of ops) {
  const r = rows.get(String(op));
  const names = [...r.names].filter((n) => n !== "Events::NoParameters" && n !== "NoParameters");
  md += `| ${op} | ${[...r.kind].join(", ")} | ${names.join("<br>")} | ${[...r.resp].join("<br>")} | ${[...r.flags].join(", ")} | ${pymax.get(op) ?? "—"} |\n`;
}

const missing = [...pymax.entries()].filter(([c]) => !rows.has(String(c))).sort((a, b) => a[0] - b[0]);
md += `\n${ops.length} desktop opcodes. In PyMax but not found as CommonPacket/CommonEvent in the desktop binary (${missing.length}):\n\n`;
md += missing.map(([c, n]) => `\`${c} ${n}\``).join(", ") + "\n";
const onlyDesktop = ops.filter((o) => !pymax.has(o));
md += `\nDesktop-only opcodes (not in PyMax, ${onlyDesktop.length}): ${onlyDesktop.join(", ")}\n`;

writeFileSync(outPath, md);
console.log(`${ops.length} opcodes, ${missing.length} PyMax-only, ${onlyDesktop.length} desktop-only`);
