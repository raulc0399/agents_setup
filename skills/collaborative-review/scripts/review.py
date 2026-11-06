#!/usr/bin/env python3
"""Run the local collaborative review coordinator."""

import sys

# Frozen runs must remain byte-for-byte unchanged when Python imports them.
sys.dont_write_bytecode = True

from review_core.cli import main


if __name__ == '__main__':
    raise SystemExit(main())
