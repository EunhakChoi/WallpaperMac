# Pinned dependency notices

Some published Cargo packages omit standalone license files even though their
manifests declare a license. The manifest here records actual upstream texts
from the VCS commit recorded by each published crate, including its source URL
and SHA-256. For hexf-parse, the pinned Cargo license declaration accompanies
the canonical Creative Commons CC0-1.0 text.

`scripts/collect-licenses.py` first copies license files shipped by Cargo. If
none exist, it checks the exact package/version/license tuple and the hashes of
these pinned files before copying them. A missing or changed notice fails the
collection. The final collector performs no network requests for this fallback.
This directory contains notices and a license declaration, not runtime code,
Wallpaper Engine resources, the Steamworks SDK or fonts.
