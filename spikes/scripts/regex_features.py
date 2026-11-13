import sys, re, pathlib, yaml, collections
root = pathlib.Path(sys.argv[1])
feat = collections.OrderedDict([
  ("lookahead (?= (?!", re.compile(r"\(\?[=!]")),
  ("lookbehind (?<= (?<!", re.compile(r"\(\?<[=!]")),
  ("backref \\1 \\k<>", re.compile(r"\\[1-9]|\\k<")),
  ("atomic (?>", re.compile(r"\(\?>")),
  ("possessive *+ ++ ?+", re.compile(r"(?<!\\)[*+?}]\+")),
  ("\\G anchor", re.compile(r"\\G")),
  ("conditional (?(", re.compile(r"\(\?\(")),
  ("\\z \\Z \\A", re.compile(r"\\[zZA]")),
])
class L(yaml.SafeLoader): pass
def ctor(loader, node): return loader.construct_scalar(node)
L.add_constructor(None, lambda l,n: None)
pats=[]; files=0; bad=0
def walk(x, vars_):
    if isinstance(x, dict):
        for k,v in x.items():
            if k=="match" and isinstance(v,str): pats.append(v)
            else: walk(v, vars_)
    elif isinstance(x, list):
        for v in x: walk(v, vars_)
for f in sorted(root.rglob("*.sublime-syntax")):
    files+=1
    try:
        d = yaml.load(f.read_text(encoding="utf-8"), Loader=L)
    except Exception as e:
        bad+=1; continue
    vars_ = d.get("variables",{}) or {}
    raw=[]; 
    before=len(pats); walk(d.get("contexts",{}), vars_)
    # expand {{vars}} recursively so features hidden in variables are counted
    def expand(p, depth=0):
        if depth>10: return p
        q = re.sub(r"\{\{(\w+)\}\}", lambda m: str(vars_.get(m.group(1), m.group(0))), p)
        return q if q==p else expand(q, depth+1)
    for i in range(before, len(pats)): pats[i]=expand(pats[i])
n=len(pats)
cnt=collections.Counter(); any_adv=0
for p in pats:
    hit=False
    for name,rx in feat.items():
        if rx.search(p): cnt[name]+=1; hit=True
    if hit: any_adv+=1
print(f"syntax files: {files} (yaml errors: {bad}); match patterns: {n}")
for name in feat: print(f"  {name:28s} {cnt[name]:6d}  {100*cnt[name]/n:5.1f}%")
print(f"  {'ANY non-DFA feature':28s} {any_adv:6d}  {100*any_adv/n:5.1f}%")
print(f"  {'DFA-compatible (approx)':28s} {n-any_adv:6d}  {100*(n-any_adv)/n:5.1f}%")
