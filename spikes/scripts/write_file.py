#!/usr/bin/env python3
"""Atomic source update, with prior bytes saved only under ignored data/backups."""
import os, pathlib, shutil, sys, time
root=pathlib.Path(__file__).resolve().parents[1]
p=(root/sys.argv[1]).resolve();p.relative_to(root)
if p.exists():
 backup=root/'data/backups'/str(time.time_ns())/p.relative_to(root)
 backup.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(p,backup)
p.parent.mkdir(parents=True,exist_ok=True)
mode=(p.stat().st_mode & 0o777) if p.exists() else None
t=p.with_name(p.name+'.tmp');t.write_text(sys.stdin.read())
if mode is not None:os.chmod(t,mode)
os.replace(t,p)
