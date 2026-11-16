#!/usr/bin/env python3
"""Disposable pair configuration for transport CI."""
import json
import os
from pathlib import Path
import sys

config, token_path, endpoint = sys.argv[1:]
config, token_path = map(Path, (config, token_path))
config.mkdir(mode=0o700, parents=True)
fd = os.open(config / 'pair.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as output:
    json.dump(dict(url=endpoint, token=token_path.read_text(), machine_id='machine-6754a9c1e83f27b4'), output)
