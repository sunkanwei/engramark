#!/bin/sh
# Engramark macOS/Linux installer. Program files are replaceable; memories stay separate.
set -eu

REPOSITORY="${ENGRAMARK_REPOSITORY:-sunkanwei/engramark}"
VERSION="${ENGRAMARK_VERSION:-}"
PACKAGE=""
EXPECTED_SHA=""
INSTALL_HOME="$HOME"

usage() {
	printf '%s\n' "用法：install.sh [--package 本地安装包] [--checksum SHA256]"
	printf '%s\n' "                 [--version 版本] [--repo GitHub账号/仓库] [--home 用户目录]"
}

fail() {
	printf '错误：%s\n' "$1" >&2
	exit 2
}

while [ "$#" -gt 0 ]; do
	case "$1" in
	--package | --checksum | --version | --repo | --home)
		[ "$#" -ge 2 ] && [ -n "$2" ] || fail "参数 $1 缺少值。"
		case "$1" in
		--package) PACKAGE="$2" ;;
		--checksum) EXPECTED_SHA="$2" ;;
		--version) VERSION="$2" ;;
		--repo) REPOSITORY="$2" ;;
		--home) INSTALL_HOME="$2" ;;
		esac
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*) fail "未知参数 $1。" ;;
	esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
Darwin-arm64) TARGET="macos-arm64" ;;
Darwin-x86_64) TARGET="macos-x86_64" ;;
Linux-x86_64) TARGET="linux-x86_64" ;;
*) fail "当前发布包不支持 $OS-$ARCH。" ;;
esac
[ -d "$INSTALL_HOME" ] || fail "用户目录不存在。"
INSTALL_HOME="$(CDPATH='' cd -- "$INSTALL_HOME" && pwd)"
[ "$INSTALL_HOME" != / ] || fail "用户目录不能是系统根目录。"
APP_ROOT="$INSTALL_HOME/.local/share/engramark"
DATA_HOME="$INSTALL_HOME/engramark"
[ ! -L "$APP_ROOT" ] || fail "程序目录不能是符号链接。"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/engramark-install.XXXXXX")"
INSTALL_LOCK=""
cleanup() {
	if [ -n "$INSTALL_LOCK" ] && [ -d "$INSTALL_LOCK" ]; then
		rm -f "$INSTALL_LOCK/pid"
		rmdir "$INSTALL_LOCK" 2>/dev/null || true
	fi
	rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT HUP INT TERM

if [ -z "$PACKAGE" ]; then
	command -v curl >/dev/null 2>&1 || fail "系统缺少 curl。"
	case "$REPOSITORY" in
	*/*) ;;
	*) fail "GitHub 仓库地址应为账号/仓库。" ;;
	esac
	if [ -n "$VERSION" ]; then
		BASE="https://github.com/$REPOSITORY/releases/download/v${VERSION#v}"
	else
		BASE="https://github.com/$REPOSITORY/releases/latest/download"
	fi
	CHECKSUMS="$TEMP_ROOT/checksums.txt"
	curl -fL --retry 3 --proto '=https' --tlsv1.2 -o "$CHECKSUMS" "$BASE/checksums.txt"
	ASSET="$(awk -v target="$TARGET" '$2 ~ ("^engramark-.*-" target "\\.tar\\.gz$") {print $2; exit}' "$CHECKSUMS")"
	[ -n "$ASSET" ] || fail "发布清单中没有 $TARGET 安装包。"
	EXPECTED_SHA="$(awk -v asset="$ASSET" '$2 == asset {print $1; exit}' "$CHECKSUMS")"
	PACKAGE="$TEMP_ROOT/$ASSET"
	curl -fL --retry 3 --proto '=https' --tlsv1.2 -o "$PACKAGE" "$BASE/$ASSET"
fi

[ -f "$PACKAGE" ] || fail "找不到安装包。"
if [ -n "$EXPECTED_SHA" ]; then
	ACTUAL_SHA="$(shasum -a 256 "$PACKAGE" | awk '{print $1}')"
	[ "$ACTUAL_SHA" = "$(printf '%s' "$EXPECTED_SHA" | tr 'A-F' 'a-f')" ] ||
		fail "安装包校验失败。"
fi
ARCHIVE_LIST="$TEMP_ROOT/archive.list"
ARCHIVE_DETAIL="$TEMP_ROOT/archive.detail"
tar -tzf "$PACKAGE" >"$ARCHIVE_LIST" || fail "安装包目录无法读取。"
tar -tvzf "$PACKAGE" >"$ARCHIVE_DETAIL" || fail "安装包元数据无法读取。"
awk '
	BEGIN { bad=0 }
	{
		name=$0; count++
		if (name ~ /[[:space:]\\]/ || name ~ /^\// ||
		    (name != "engramark" && name !~ /^engramark\//) ||
		    name ~ /(^|\/)\.\.?($|\/)/ || name ~ /\/\//) bad=1
		fold=tolower(name); if (seen[fold]++) bad=1
	}
	END { if (count == 0 || count > 4096 || bad) exit 1 }
' "$ARCHIVE_LIST" || fail "安装包包含不安全、重复或超量路径。"
SIZE_FIELD=5
[ "$OS" = Linux ] && SIZE_FIELD=3
awk -v size_field="$SIZE_FIELD" '
	BEGIN { total=0 }
	{
		type=substr($1,1,1); size=$size_field+0
		if (type != "-" && type != "d") exit 1
		if (size > 268435456) exit 1
		total += size; if (total > 536870912) exit 1
	}
' "$ARCHIVE_DETAIL" || fail "安装包包含链接、特殊文件或超量内容。"
tar -xzf "$PACKAGE" -C "$TEMP_ROOT"
STAGE="$TEMP_ROOT/engramark"
[ -x "$STAGE/bin/engramark" ] || fail "安装包缺少原生二进制。"
[ -f "$STAGE/MANIFEST.tsv" ] || fail "安装包缺少逐文件清单。"
EXPECTED_LIST="$TEMP_ROOT/expected.list"
ACTUAL_LIST="$TEMP_ROOT/actual.list"
awk -F '\t' '
	NF != 4 || ($1 != "d" && $1 != "f") || $2 !~ /^[0-9]+$/ ||
	$4 == "" || $4 == "MANIFEST.tsv" || $4 ~ /[[:space:]\\]/ ||
	$4 ~ /(^|\/)\.\.?($|\/)/ || $4 ~ /^\// { exit 1 }
	{ print $4 }
' "$STAGE/MANIFEST.tsv" >"$EXPECTED_LIST" || fail "逐文件清单格式非法。"
printf '%s\n' MANIFEST.tsv >>"$EXPECTED_LIST"
LC_ALL=C sort -o "$EXPECTED_LIST" "$EXPECTED_LIST"
(CDPATH='' cd -- "$STAGE" && find . -mindepth 1 -print | sed 's#^\./##' | LC_ALL=C sort) >"$ACTUAL_LIST"
cmp -s "$EXPECTED_LIST" "$ACTUAL_LIST" || fail "安装包存在清单外条目或缺少声明条目。"
TAB="$(printf '\t')"
while IFS="$TAB" read -r KIND SIZE DIGEST RELATIVE; do
	TARGET_PATH="$STAGE/$RELATIVE"
	if [ "$KIND" = d ]; then
		[ -d "$TARGET_PATH" ] && [ ! -L "$TARGET_PATH" ] || fail "清单目录非法：$RELATIVE"
		continue
	fi
	[ -f "$TARGET_PATH" ] && [ ! -L "$TARGET_PATH" ] || fail "清单文件非法：$RELATIVE"
	ACTUAL_SIZE="$(wc -c <"$TARGET_PATH" | tr -d '[:space:]')"
	[ "$ACTUAL_SIZE" = "$SIZE" ] || fail "文件大小校验失败：$RELATIVE"
	ACTUAL_FILE_SHA="$(shasum -a 256 "$TARGET_PATH" | awk '{print $1}')"
	[ "$ACTUAL_FILE_SHA" = "$DIGEST" ] || fail "逐文件校验失败：$RELATIVE"
done <"$STAGE/MANIFEST.tsv"
PACKAGE_VERSION="$(sed -n '1p' "$STAGE/VERSION")"
if [ -n "$VERSION" ] && [ "${VERSION#v}" != "$PACKAGE_VERSION" ]; then
	fail "安装包版本与指定版本不一致。"
fi

# Binary self-check: capability probe and version agreement before touching the host.
ENGRAMARK_HOME="$TEMP_ROOT/selfcheck" "$STAGE/bin/engramark" rebuild >/dev/null ||
	fail "二进制自检失败（SQLite 能力探针未通过）。"
rm -rf "$TEMP_ROOT/selfcheck"

"$STAGE/bin/engramark" host-setup check \
	--home "$INSTALL_HOME" --app-root "$APP_ROOT" --data-home "$DATA_HOME"

mkdir -p "$(dirname "$APP_ROOT")"
INSTALL_LOCK="$APP_ROOT.install.lock"
if ! mkdir "$INSTALL_LOCK" 2>/dev/null; then
	LOCK_PID="$(sed -n '1p' "$INSTALL_LOCK/pid" 2>/dev/null || true)"
	case "$LOCK_PID" in
	'' | *[!0-9]*) fail "另一个安装进程正在运行，或安装锁需要人工检查：$INSTALL_LOCK" ;;
	*)
		if kill -0 "$LOCK_PID" 2>/dev/null; then
			fail "另一个安装进程正在运行（PID $LOCK_PID）。"
		fi
		rm -f "$INSTALL_LOCK/pid"
		rmdir "$INSTALL_LOCK" 2>/dev/null || fail "安装锁需要人工检查：$INSTALL_LOCK"
		mkdir "$INSTALL_LOCK" || fail "无法获取安装锁。"
		;;
	esac
fi
chmod 700 "$INSTALL_LOCK"
printf '%s\n' "$$" >"$INSTALL_LOCK/pid"
chmod 600 "$INSTALL_LOCK/pid"
PREVIOUS=""
if [ -e "$APP_ROOT" ]; then
	PREVIOUS="$APP_ROOT.previous-$$"
	mv "$APP_ROOT" "$PREVIOUS"
fi
restore() {
	if [ -e "$APP_ROOT" ] || [ -L "$APP_ROOT" ]; then
		rm -rf "$APP_ROOT"
	fi
	if [ -n "$PREVIOUS" ] && [ -d "$PREVIOUS" ]; then
		mv "$PREVIOUS" "$APP_ROOT"
	fi
}
if ! mv "$STAGE" "$APP_ROOT"; then
	restore
	fail "程序目录切换失败，已恢复旧版本。"
fi
chmod 700 "$APP_ROOT"

if ! ENGRAMARK_HOME="$DATA_HOME" "$APP_ROOT/bin/install.sh"; then
	restore
	fail "数据目录初始化失败，已恢复旧版本。"
fi
if ! ENGRAMARK_HOME="$DATA_HOME" "$APP_ROOT/bin/engramark" search "" >/dev/null 2>&1; then
	restore
	fail "安装后冒烟失败，已恢复旧版本。"
fi
if ! "$APP_ROOT/bin/engramark" host-setup install \
	--home "$INSTALL_HOME" --app-root "$APP_ROOT" --data-home "$DATA_HOME"; then
	restore
	fail "宿主接线失败，已恢复安装前状态。"
fi
if [ -n "$PREVIOUS" ]; then
	if ! rm -rf "$PREVIOUS"; then
		printf '警告：旧程序备份未能清理，请人工检查：%s\n' "$PREVIOUS" >&2
	fi
fi

printf '\n安装完成：Engramark %s\n' "$PACKAGE_VERSION"
printf '程序目录：%s\n' "$APP_ROOT"
printf '记忆目录：%s（重装不会覆盖）\n' "$DATA_HOME"
printf '命令入口：%s/bin/engramark\n' "$APP_ROOT"
printf '\n如 Codex 或 OpenCode 正在运行，请重启宿主以加载新二进制；\n'
printf '仍在运行的旧会话可能引用已被替换的旧程序路径。\n'
