#!/usr/bin/env bash
set -euo pipefail

# Preserve the supplied toolkit's terms inside the tree copied into the slim
# image. The vendor's Debian copyright file lives outside that tree, and its
# doc/EULA.txt (when present) is otherwise removed with the documentation.
toolkit_version="${1:?expected CUDA toolkit version}"
toolkit_root="${2:-/usr/local/cuda}"
doc_root="${3:-/usr/share/doc}"
package_version="${toolkit_version%.*}"
package_version="${package_version//./-}"

for candidate in \
    "$toolkit_root/EULA.txt" \
    "$toolkit_root/LICENSE" \
    "$toolkit_root/LICENSE.txt" \
    "$toolkit_root/doc/EULA.txt" \
    "$doc_root/cuda-cudart-$package_version/copyright"; do
    if [[ -s "$candidate" ]]; then
        if [[ "$candidate" != "$toolkit_root/EULA.txt" ]]; then
            cp "$candidate" "$toolkit_root/EULA.txt"
        fi
        exit 0
    fi
done

echo "CUDA $toolkit_version redistribution license material was not found in the supplied toolkit" >&2
exit 1
