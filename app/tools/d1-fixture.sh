#!/bin/sh
# d1-fixture.sh DIR : writes the D1 probe fixture into DIR (created; must
# not exist yet). All content is made up:
#   25 top-level .md notes, 3 other files, one nested folder with 5 .md,
#   and one hidden .md the probe must skip.
# Expected count: md_top=25 md_total=30 other=3 folders=1.
set -eu
dir=${1:?usage: d1-fixture.sh DIR}
if [ -e "$dir" ]; then
  echo "d1-fixture: $dir exists; pick a new path" >&2
  exit 1
fi
mkdir -p "$dir/nested"
n=1
while [ "$n" -le 25 ]; do
  name=$(printf 'note-%02d.md' "$n")
  printf '# Fixture note %d\n\nMade-up text for the folder probe.\n' "$n" > "$dir/$name"
  n=$((n + 1))
done
printf 'made up\n' > "$dir/readme.txt"
printf '{}\n' > "$dir/data.json"
printf 'not an image\n' > "$dir/image.png"
n=1
while [ "$n" -le 5 ]; do
  printf '# Nested note %d\n' "$n" > "$dir/nested/deep-$n.md"
  n=$((n + 1))
done
printf 'hidden, skipped\n' > "$dir/.hidden.md"
echo "d1-fixture: wrote $dir (expect md_top=25 md_total=30 other=3 folders=1)"
