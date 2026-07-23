#!/bin/sh
set -eu

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${HOME}/.local/bin
data_dir=${XDG_DATA_HOME:-${HOME}/.local/share}
applications_dir=${data_dir}/applications
scalable_icons_dir=${data_dir}/icons/hicolor/scalable/apps
png_icons_dir=${data_dir}/icons/hicolor/512x512/apps

install -d "$bin_dir" "$applications_dir" "$scalable_icons_dir" "$png_icons_dir"
install -m 755 "$package_dir/weiagent" "$bin_dir/weiagent"
install -m 644 "$package_dir/weiagent.svg" "$scalable_icons_dir/weiagent.svg"
install -m 644 "$package_dir/weiagent.png" "$png_icons_dir/weiagent.png"

escaped_executable=$(printf '%s\n' "$bin_dir/weiagent" | sed 's/[&|]/\\&/g')
sed "s|@EXECUTABLE@|$escaped_executable|g" "$package_dir/weiagent.desktop.in" \
  > "$applications_dir/weiagent.desktop"
chmod 644 "$applications_dir/weiagent.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$applications_dir" >/dev/null 2>&1 || true
fi

printf 'WeiAgent installed to %s\n' "$bin_dir/weiagent"
