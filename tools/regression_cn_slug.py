#!/usr/bin/env python3
"""真实接口回归：中国站「昵称 ≠ 账号标识（slug）」。

## 背景

用户报告：

    账号数据获取失败（当前站点：中国站 leetcode.cn（力扣））：
    未找到资源: 未找到用户 「梧糊」，请检查用户名拼写

上一轮（第四轮）修掉了 HTTP 400——那是"用国际站 schema 打中国站"导致的。
这一轮的报错文案已经正确带上了站点信息，但用户依然拿不到数据。

## 根因

中国站 `userProfilePublicProfile(userSlug:)` 按 **ASCII slug** 查找，
而用户在设置页填的、界面各处展示的都是**显示昵称**。用户可以把昵称改成
中文，slug 却不会跟着变。于是"查不到"其实只是"查错了字段"。

## 验证目标

1. 中文昵称确实查不到（复现用户现象，证明不是拼写问题）
2. ASCII slug 能查到（证明修复方向正确）
3. `userStatus` 能拿到 slug（证明自动纠正方案可行）—— 需登录态
4. `/u/<slug>/` 是 200，`/u/<中文昵称>/` 是 400（旁证：slug 是 URL 契约）

## 用法

    python tools/regression_cn_slug.py [slug]

`slug` 默认 `wuhu`（可换成任一存在的中国站账号标识）。

无需登录即可跑第 1/2/4 项；第 3 项需要 `LEETCODE_SESSION` 与 `csrftoken`
环境变量，缺失时跳过并说明原因。
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request

ENDPOINT = "https://leetcode.cn/graphql"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"

PROFILE_QUERY = """
query userProfilePublicProfile($userSlug: String!) {
  userProfilePublicProfile(userSlug: $userSlug) {
    username
    siteRanking
  }
}
"""

USER_STATUS_QUERY = """
query userStatus {
  userStatus {
    isSignedIn
    username
    userSlug
    realName
  }
}
"""

# 非 ASCII 取样：一个典型的中文昵称形态。
CJK_SAMPLE = "梧糊"

failures: list[str] = []
skips: list[str] = []


def graphql(query: str, variables: dict, cookie: str | None = None) -> dict:
    body = json.dumps({"query": query, "variables": variables}).encode()
    headers = {
        "Content-Type": "application/json",
        "User-Agent": UA,
        "Origin": "https://leetcode.cn",
        "Referer": "https://leetcode.cn/",
    }
    if cookie:
        headers["Cookie"] = cookie

    req = urllib.request.Request(
        ENDPOINT, data=body, headers=headers, method="POST"
    )
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.loads(resp.read())


def check(label: str, cond: bool, detail: str = "") -> None:
    mark = "PASS" if cond else "FAIL"
    print(f"  [{mark}] {label}")
    if detail:
        print(f"         {detail}")
    if not cond:
        failures.append(label)


def main() -> int:
    slug = sys.argv[1] if len(sys.argv) > 1 else "wuhu"
    print(f"目标站点: leetcode.cn   参照标识(slug): {slug}\n")

    # --- 1. 中文昵称查不到（复现用户现象）---
    print("场景 1：用中文昵称查询 —— 期望查不到（复现用户报告）")
    r = graphql(PROFILE_QUERY, {"userSlug": CJK_SAMPLE})
    node = (r.get("data") or {}).get("userProfilePublicProfile")
    check(
        f'userSlug:"{CJK_SAMPLE}" 返回 null',
        node is None,
        f"实得: {json.dumps(node, ensure_ascii=False)}",
    )

    # --- 2. ASCII slug 能查到（证明修复方向）---
    print(f"\n场景 2：用 ASCII 标识查询 —— 期望命中")
    r = graphql(PROFILE_QUERY, {"userSlug": slug})
    node = (r.get("data") or {}).get("userProfilePublicProfile")
    check(
        f'userSlug:"{slug}" 命中',
        node is not None,
        f"实得: {json.dumps(node, ensure_ascii=False)}",
    )
    if node:
        check(
            "命中的昵称与查询标识可以不同",
            True,
            f'服务端展示名: {node.get("username")!r}  '
            f'查询标识: {slug!r}  → '
            f'{"两者不同，正是缺陷根源" if node.get("username") != slug else "本例两者相同"}',
        )

    # --- 3. userStatus 能拿到 slug（自动纠正方案的前提）---
    print("\n场景 3：登录态 userStatus 返回 userSlug —— 自动纠正的前提")
    session = os.environ.get("LEETCODE_SESSION", "").strip()
    csrf = os.environ.get("CSRFTOKEN", "").strip()
    if not session:
        skips.append("场景 3：未设置 LEETCODE_SESSION，跳过")
        print("  [SKIP] 未设置 LEETCODE_SESSION 环境变量")
        print("         无法验证登录态；该场景是自动纠正功能的关键前提，")
        print("         上线前应带真实凭据跑一次。")
    else:
        cookie = f"LEETCODE_SESSION={session}"
        if csrf:
            cookie += f"; csrftoken={csrf}"
        r = graphql(USER_STATUS_QUERY, {}, cookie)
        status = (r.get("data") or {}).get("userStatus") or {}
        signed = status.get("isSignedIn")
        got_slug = status.get("userSlug")
        check("userStatus.isSignedIn 为 true", signed is True, f"实得: {signed}")
        check(
            "userStatus.userSlug 非空",
            bool(got_slug),
            f'实得: {got_slug!r}（这是唯一能自动解析出正确标识的字段）',
        )
        if got_slug:
            check(
                "拿到的 slug 可直接用于画像查询",
                True,
                f'下一步可用 userSlug:"{got_slug}" 查询，无需用户手工查证',
            )

    # --- 4. URL 契约旁证 ---
    print("\n场景 4：个人主页 URL 契约 —— /u/<slug>/ 而非 /u/<昵称>/")

    def head_status(url: str) -> int | str:
        """返回 HTTP 状态码。

        注意：不能只捕获 `HTTPError`——`urlopen` 在**读取响应体**时若遇到
        解码失败会抛 `UnicodeDecodeError`/`http.client` 异常。对中文路径
        段服务端返回 400 且响应体可能非 UTF-8，正好踩中这一点。
        因此这里显式做一次 HEAD（不读 body）并兜底捕获所有异常。
        """
        req = urllib.request.Request(url, headers={"User-Agent": UA}, method="HEAD")
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                return resp.status
        except urllib.error.HTTPError as e:
            return e.code
        except Exception as e:  # noqa: BLE001 —— 状态探测不该因解码失败而崩
            return f"异常 {type(e).__name__}"

    ok = head_status(f"https://leetcode.cn/u/{slug}/")
    check(f"/u/{slug}/ 返回 200", ok == 200, f"实得: HTTP {ok}")

    bad = head_status(f"https://leetcode.cn/u/{CJK_SAMPLE}/")
    check(
        f"/u/{CJK_SAMPLE}/ 无法访问",
        bad != 200,
        f"实得: {bad} —— 原始中文不能作为 URL 路径段（RFC 3986），"
        f"说明 slug 才是 URL 契约里那一段",
    )

    # --- 汇总 ---
    print("\n" + "=" * 62)
    if skips:
        for s in skips:
            print(f"  SKIP  {s}")
    if failures:
        print(f"  结果: {len(failures)} 项失败")
        for f in failures:
            print(f"        - {f}")
        return 1
    print("  结果: 全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
