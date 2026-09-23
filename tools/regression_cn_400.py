"""End-to-end regression test for the reported bug.

User report (verbatim):

    账号数据获取失败：getUserProfile 返回异常状态码 HTTP 400，用户名为梧糊

Root cause: the CN site's GraphQL schema has no `matchedUser` field, so the
international-site query returned HTTP 400. The Chinese username was a red
herring — ASCII usernames failed identically on leetcode.cn.

This script replays BOTH the old (broken) and new (fixed) queries against
the live CN endpoint to prove:
  1. the old query still fails with exactly the reported error,
  2. the new query succeeds,
  3. the Chinese username is not the cause.
"""

import json
import re
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
QUERIES_RS = ROOT / "src" / "leetcode" / "queries.rs"
CN_ENDPOINT = "https://leetcode.cn/graphql"

CN_HEADERS = {
    "Content-Type": "application/json",
    "Origin": "https://leetcode.cn",
    "Referer": "https://leetcode.cn/problemset/",
    "User-Agent": (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
        "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
    ),
}


def load_query(const_name):
    text = QUERIES_RS.read_text(encoding="utf-8")
    m = re.search(
        r"pub const " + const_name + r": &str = r#\"(.*?)\"#;", text, re.DOTALL
    )
    if not m:
        raise SystemExit(f"未找到常量 {const_name}")
    return m.group(1)


def post(query, variables, operation_name):
    payload = {
        "query": query,
        "variables": variables,
        "operationName": operation_name,
    }
    req = urllib.request.Request(
        CN_ENDPOINT,
        data=json.dumps(payload).encode("utf-8"),
        headers=CN_HEADERS,
        method="POST",
    )
    try:
        with urllib.request.urlopen(req) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")


def main():
    failures = []

    print("=" * 68)
    print("回归测试：复现用户报告的缺陷，并验证修复")
    print("=" * 68)

    print("\n[场景 1] 旧查询（国际站字段）打中国站 —— 应复现 HTTP 400")
    old_q = load_query("USER_PROFILE")  # 旧的、写死 matchedUser 的查询
    status, body = post(old_q, {"username": "梧糊"}, "getUserProfile")
    print(f"  HTTP {status}")
    print(f"  {body[:200]}")
    if status == 400:
        print("  -> 复现成功：这与用户看到的错误一致")
    else:
        failures.append(f"场景 1 未复现 400，实得 {status}")

    print("\n[场景 2] 旧查询 + 纯 ASCII 用户名 —— 应同样 400")
    status2, body2 = post(old_q, {"username": "leetcode"}, "getUserProfile")
    print(f"  HTTP {status2}")
    print(f"  {body2[:200]}")
    if status2 == 400:
        print("  -> 证明中文用户名不是根因：ASCII 用户名同样失败")
    else:
        failures.append(f"场景 2 未复现 400，实得 {status2}")

    print("\n[场景 3] 新查询（中国站字段）+ 中文用户名 —— 应成功")
    new_profile = load_query("CN_USER_PROFILE")
    new_progress = load_query("CN_USER_PROGRESS")
    s3a, b3a = post(new_profile, {"userSlug": "梧糊"}, "userProfilePublicProfile")
    s3b, b3b = post(new_progress, {"userSlug": "梧糊"}, "userProfileUserQuestionProgressV2")
    print(f"  画像   HTTP {s3a}  {b3a[:130]}")
    print(f"  进度   HTTP {s3b}  {b3b[:130]}")
    if s3a == 200 and s3b == 200:
        print("  -> 修复生效：中文用户名不再触发 400")
    else:
        failures.append(f"场景 3 失败：{s3a} / {s3b}")

    print("\n[场景 4] 新查询 + 真实存在用户 —— 应返回完整数据")
    s4a, b4a = post(new_profile, {"userSlug": "leetcode"}, "userProfilePublicProfile")
    s4b, b4b = post(new_progress, {"userSlug": "leetcode"}, "userProfileUserQuestionProgressV2")
    j4a = json.loads(b4a)
    j4b = json.loads(b4b)
    prof = (j4a.get("data") or {}).get("userProfilePublicProfile") or {}
    prog = (j4b.get("data") or {}).get("userProfileUserQuestionProgressV2") or {}
    print(f"  HTTP {s4a} / {s4b}")
    print(f"  username   = {prof.get('username')!r}")
    print(f"  siteRanking= {prof.get('siteRanking')!r}")
    print(f"  reputation = {(prof.get('profile') or {}).get('reputation')!r}")
    print(f"  accepted   = {prog.get('numAcceptedQuestions')!r}")
    if prof.get("username") and prog.get("numAcceptedQuestions"):
        print("  -> 数据链路完整")
    else:
        failures.append("场景 4 数据不完整")

    print("\n[场景 5] 新题库查询 —— 应返回 total 与题目")
    prob_q = load_query("CN_PROBLEM_LIST")
    s5, b5 = post(
        prob_q,
        {"categorySlug": "", "skip": 0, "limit": 2, "filters": {}},
        "problemsetQuestionList",
    )
    j5 = json.loads(b5)
    page = (j5.get("data") or {}).get("problemsetQuestionList") or {}
    print(f"  HTTP {s5}  total={page.get('total')}  hasMore={page.get('hasMore')}")
    first = (page.get("questions") or [{}])[0]
    print(
        f"  首题 frontendQuestionId={first.get('frontendQuestionId')!r} "
        f"difficulty={first.get('difficulty')!r}"
    )
    if s5 == 200 and page.get("total") and first.get("frontendQuestionId"):
        print("  -> 题库接口正常")
    else:
        failures.append("场景 5 题库接口异常")

    print("\n" + "=" * 68)
    if failures:
        for f in failures:
            print(f"失败：{f}")
        raise SystemExit(1)
    print("全部场景通过：缺陷已复现并确认修复")


if __name__ == "__main__":
    main()
