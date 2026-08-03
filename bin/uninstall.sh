#!/bin/sh
# Remove the program and host wiring. Memories are always retained.
set -eu

INSTALL_HOME="$HOME"
while [ "$#" -gt 0 ]; do
	case "$1" in
	--home)
		if [ "$#" -lt 2 ] || [ -z "$2" ]; then
			printf '%s\n' "错误：参数 --home 缺少值。" >&2
			exit 2
		fi
		INSTALL_HOME="$2"
		shift 2
		;;
	-h | --help)
		printf '%s\n' "用法：uninstall [--home 用户目录]"
		exit 0
		;;
	*)
		printf '%s\n' "错误：未知参数 $1" >&2
		exit 2
		;;
	esac
done

[ -d "$INSTALL_HOME" ] || {
	printf '%s\n' "错误：用户目录不存在。" >&2
	exit 2
}
INSTALL_HOME="$(CDPATH='' cd -- "$INSTALL_HOME" && pwd)"
[ "$INSTALL_HOME" != / ] || {
	printf '%s\n' "错误：用户目录不能是系统根目录。" >&2
	exit 2
}
APP_ROOT="$INSTALL_HOME/.local/share/engramark"
DATA_HOME="$INSTALL_HOME/engramark"
SOURCE_ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
BINARY="$SOURCE_ROOT/bin/engramark"

[ -x "$BINARY" ] || {
	printf '%s\n' "错误：卸载程序缺少原生二进制。" >&2
	exit 2
}
[ ! -L "$APP_ROOT" ] || {
	printf '%s\n' "错误：程序目录是符号链接，已停止卸载。" >&2
	exit 2
}

"$BINARY" host-setup uninstall \
	--home "$INSTALL_HOME" --app-root "$APP_ROOT" --data-home "$DATA_HOME"

if [ -d "$APP_ROOT" ]; then
	rm -rf "$APP_ROOT"
fi
printf '程序与宿主接线已移除。记忆始终保留在：%s\n' "$DATA_HOME"
