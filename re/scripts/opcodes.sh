#!/usr/bin/env bash
# Extract the desktop opcode table from C++ template names embedded in core.dll:
#   CommonPacket<OPCODE,struct Api::OneMe::Packets::NAME::(Parameters|Payload),struct ...::RESPONSE,FLAG>
#   CommonEvent<OPCODE,struct ...,struct Api::OneMe::Packets::Events::NAME::Response...
# Output TSV: opcode, kind (packet|event), request type, response type, flag.
# Usage: opcodes.sh <core.dll> <out.tsv>
set -euo pipefail
bin=$1; out=$2
tab=$'\t'
p='Api::OneMe::Packets::'
rg -a -o -N "CommonPacket<[0-9]+,(struct|class) ${p}[A-Za-z0-9_:]+,(struct|class) ${p}[A-Za-z0-9_:]+,[0-9]+" "$bin" \
  | sed -E "s/^CommonPacket<([0-9]+),(struct|class) ${p}([A-Za-z0-9_:]+),(struct|class) ${p}([A-Za-z0-9_:]+),([0-9]+)\$/\1${tab}packet${tab}\3${tab}\5${tab}\6/" \
  > "$out.tmp"
rg -a -o -N "CommonEvent<[0-9]+,(struct|class) ${p}[A-Za-z0-9_:]+,(struct|class) ${p}[A-Za-z0-9_:]+" "$bin" \
  | sed -E "s/^CommonEvent<([0-9]+),(struct|class) ${p}([A-Za-z0-9_:]+),(struct|class) ${p}([A-Za-z0-9_:]+)\$/\1${tab}event${tab}\3${tab}\5${tab}/" \
  >> "$out.tmp"
sort -t "$tab" -k1,1n -k2,2 -k3,3 -u "$out.tmp" > "$out"
rm -f "$out.tmp"
echo "rows: $(wc -l < "$out"), opcodes: $(cut -f1 "$out" | sort -u | wc -l)"
