#!/bin/sh
# olo COMMAND [ARGUMENT…]: a command of the Olo desktop this Rex host is
# used from, asked through Rex (olo.lua beside this file, run by
# `rex do`): olo open URL, olo op …, olo screen …, olo notify TITLE
# [BODY], olo clipboard TEXT. Part of Olo's skill for agents.
here=$(dirname "$0")
OLO_BRIDGE_CMD=${1:-}
[ $# -gt 0 ] && shift
i=0
for a in "$@"; do i=$((i + 1)); export "OLO_BRIDGE_ARG_$i=$a"; done
OLO_BRIDGE_ARGC=$i
OLO_BRIDGE_DIR="${TMPDIR:-/var/tmp}/olo-rex-bridge"
export OLO_BRIDGE_CMD OLO_BRIDGE_ARGC OLO_BRIDGE_DIR
mkdir -p "$OLO_BRIDGE_DIR" 2> /dev/null || true
# (OLO_BRIDGE_REX: another way to say `rex`, for Olo's own tests.)
exec ${OLO_BRIDGE_REX:-rex} --timeout 60s do "$here/olo.lua"
