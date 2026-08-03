#!/bin/sh
# Prepare the private data directory for the Engramark binary.
set -eu

[ "$#" -eq 0 ] || {
	printf '%s\n' "用法：bin/install.sh" >&2
	exit 2
}

SOURCE_ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
DATA_HOME="${ENGRAMARK_HOME:-$HOME/engramark}"
BINARY="$SOURCE_ROOT/bin/engramark"

[ -x "$BINARY" ] || {
	printf '%s\n' "错误：安装包缺少原生二进制。" >&2
	exit 1
}
case "$DATA_HOME" in
"" | / | "$HOME")
	printf '%s\n' "错误：记忆目录不安全。" >&2
	exit 2
	;;
esac

mkdir -p "$DATA_HOME"/cards "$DATA_HOME"/state/transactions \
	"$DATA_HOME"/state/locks "$DATA_HOME"/cache "$DATA_HOME"/logs
chmod 700 "$DATA_HOME"
if [ ! -f "$DATA_HOME/engramark.json" ]; then
	cp "$SOURCE_ROOT/engramark.json" "$DATA_HOME/engramark.json"
	chmod 600 "$DATA_HOME/engramark.json"
fi

ENGRAMARK_HOME="$DATA_HOME" "$BINARY" migrate-v1
ENGRAMARK_HOME="$DATA_HOME" "$BINARY" rebuild >/dev/null
ENGRAMARK_HOME="$DATA_HOME" "$BINARY" diagnose --full >/dev/null

printf '数据目录已就绪：%s\n' "$DATA_HOME"
