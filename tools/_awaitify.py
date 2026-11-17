"""一次性辅助脚本：给 tools.rs 中同步的仓储调用补 .await（用后即删）。"""
import re
import sys

path = sys.argv[1]
src = open(path, encoding="utf-8").read()

# 纯函数化后的时间辅助：去掉多余的 db 参数
src = src.replace("year(db)?", "year()").replace("today(db)?", "today()")
src = src.replace("now_hhmm(db)?", "now_hhmm()").replace("month_now(db)?", "month_now()")
src = src.replace("today(db).", "today().").replace("year(db).", "year().")
src = src.replace("now_hhmm(db).", "now_hhmm().").replace("month_now(db).", "month_now().")

# next_id 从方法改为自由函数
src = src.replace("db.next_id(", "next_id(db, ")

call_re = re.compile(
    r"\b db\.[A-Za-z_][A-Za-z0-9_]*\( | \b(?:exists|dept_of|add_event|notify|add_flow|next_id)\(",
    re.X,
)

matches = list(call_re.finditer(src))
inserts = []
for m in reversed(matches):
    open_paren = src.index("(", m.start())
    depth = 0
    quote = None
    esc = False
    k = open_paren
    while k < len(src):
        ch = src[k]
        if quote:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == quote:
                quote = None
        else:
            if ch in "\"'":
                quote = ch
            elif ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0:
                    break
        k += 1
    if k >= len(src):
        raise SystemExit(f"unbalanced paren near offset {m.start()}")
    rest = src[k + 1:]
    if rest.lstrip().startswith(".await"):
        continue
    inserts.append(k + 1)

for pos in inserts:
    src = src[:pos] + ".await" + src[pos:]

open(path, "w", encoding="utf-8", newline="\n").write(src)
print(f"inserted {len(inserts)} awaits")
