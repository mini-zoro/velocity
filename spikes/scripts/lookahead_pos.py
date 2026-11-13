import sys, re, pathlib, yaml, collections
root = pathlib.Path(sys.argv[1])
class L(yaml.SafeLoader): pass
L.add_constructor(None, lambda l,n: None)
def strip_x(p):
    # remove (?x) comments/whitespace approx
    if p.lstrip().startswith("(?x)"):
        out=[]; i=0; cls=False
        while i<len(p):
            c=p[i]
            if c=="\\": out.append(p[i:i+2]); i+=2; continue
            if c=="[": cls=True
            elif c=="]": cls=False
            if not cls and c=="#":
                while i<len(p) and p[i]!="\n": i+=1
                continue
            if not cls and c.isspace(): i+=1; continue
            out.append(c); i+=1
        p="".join(out)
    return p
def top_groups(p):
    """return list of (start,end,kind) for top-level groups; kind is the group prefix"""
    res=[]; depth=0; i=0; cls=False; start=None
    while i<len(p):
        c=p[i]
        if c=="\\": i+=2; continue
        if cls:
            if c=="]": cls=False
            i+=1; continue
        if c=="[": cls=True
        elif c=="(":
            if depth==0: start=i
            depth+=1
        elif c==")":
            depth-=1
            if depth==0: res.append((start,i+1,p[start:start+3]))
        i+=1
    return res
pats=[]
for f in sorted(root.rglob("*.sublime-syntax")):
    d=yaml.load(f.read_text(encoding="utf-8"),Loader=L)
    vars_=d.get("variables",{}) or {}
    def expand(p,k=0):
        q=re.sub(r"\{\{(\w+)\}\}",lambda m:str(vars_.get(m.group(1),m.group(0))),p)
        return q if q==p or k>10 else expand(q,k+1)
    def walk(x):
        if isinstance(x,dict):
            for k,v in x.items():
                if k=="match" and isinstance(v,str): pats.append(expand(v))
                else: walk(v)
        elif isinstance(x,list):
            for v in x: walk(v)
    walk(d.get("contexts",{}))
LA=re.compile(r"\(\?[=!]")
cat=collections.Counter()
for p in pats:
    if not LA.search(p): continue
    q=strip_x(p)
    if re.fullmatch(r"\(\?[=!].*\)",q,re.S) and len(top_groups(q))==1:
        cat["pure lookahead (whole pattern)"]+=1; continue
    g=top_groups(q)
    # remove trailing top-level lookahead groups
    end=len(q)
    while g and g[-1][1]==end and g[-1][2][:3] in ("(?=","(?!"):
        end=g[-1][0]; g=g[:-1]
    # remove leading top-level lookahead groups
    beg=0
    gg=top_groups(q[:end])
    while gg and gg[0][0]==beg and gg[0][2][:3] in ("(?=","(?!"):
        beg=gg[0][1]; gg=gg[1:]
    rest=q[beg:end]
    if not LA.search(rest):
        cat["only leading/trailing lookahead"]+=1
    else:
        cat["lookahead nested inside pattern"]+=1
tot=sum(cat.values())
for k,v in cat.most_common(): print(f"{k:36s} {v:5d} {100*v/tot:5.1f}%")
print("total with lookahead:",tot)
