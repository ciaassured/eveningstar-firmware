"""Fail if any requested Kconfig value did not survive resolution.

Kconfig silently ignores unknown symbols, drops values whose dependencies are
not met, and lets one choice win over another. Compare every requested value
against the resolved sdkconfig.json so those mistakes break the build instead.
"""

import json
import sys

requested = json.load(open(sys.argv[1]))
resolved = json.load(open(sys.argv[2]))

errors = []
for key, want in sorted(requested.items()):
    if key not in resolved:
        errors.append(f"CONFIG_{key}: not a visible symbol (typo, or its dependencies are not met)")
        continue

    got = resolved[key]
    # Hex values are requested as strings but resolve to ints.
    if isinstance(want, str) and type(got) is int:
        try:
            want = int(want, 0)
        except ValueError:
            pass

    if type(got) is not type(want) or got != want:
        errors.append(f"CONFIG_{key}: requested {want!r}, resolved to {got!r}")

if errors:
    print("sdkconfig check failed:", file=sys.stderr)
    for error in errors:
        print(f"  {error}", file=sys.stderr)
    sys.exit(1)

print(f"sdkconfig check passed ({len(requested)} values)")
