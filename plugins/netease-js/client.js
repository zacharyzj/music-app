// 仅请求网易云自己的接口。Cookie 不会交给音频 CDN，也不打印到日志。
import { t } from "./i18n.js";
const ORIGIN = "https://interfacepc.music.163.com";
const KEY = "e82ckenh8dichen8";
const SEPARATOR = "-36cd479b6b5-";

export function fail(code, message, retryable = false) {
  throw Object.assign(new Error(t(message)), { code, retryable });
}

export function id(value) {
  const s = String(value ?? "");
  if (!/^\d+$/.test(s)) fail("unavailable", "无效的网易云资源编号");
  return s;
}

function pairs(text) {
  const jar = {};
  for (const part of String(text || "").split(";")) {
    const i = part.indexOf("=");
    if (i > 0) jar[part.slice(0, i).trim()] = part.slice(i + 1).trim();
  }
  return jar;
}

export function session() { return host.storage.get("cookies") || {}; }
export function hasSession() { return Boolean(session().MUSIC_U); }

export function resetSession() {
  host.storage.set("sessionEpoch", (host.storage.get("sessionEpoch") || 0) + 1);
  for (const key of ["cookies", "account", "likedIds", "likedAt"]) host.storage.remove(key);
}

export function requireAccount() {
  const account = host.storage.get("account");
  if (!hasSession() || !account) fail("signIn", "请先登录网易云音乐");
  return account;
}

export async function api(path, data = {}, options = {}) {
  if (!/^\/api\/[A-Za-z0-9/_-]+$/.test(path)) fail("failed", "无效的接口路径");
  const jar = options.cookies ?? session();
  const epoch = host.storage.get("sessionEpoch") || 0;
  let deviceId = host.storage.get("deviceId");
  if (!deviceId) {
    deviceId = host.crypto.md5(`${Date.now()}:${Math.random()}`);
    host.storage.set("deviceId", deviceId);
  }
  const header = {
    os: "pc", appver: "3.1.11", deviceId, versioncode: "140",
    osver: "Microsoft-Windows-10-Professional-build-19045-64bit",
    channel: "netease", resolution: "1920x1080", __csrf: jar.__csrf || "",
    requestId: `${Date.now()}_${Math.floor(Math.random() * 10000)}`,
    ...jar,
  };
  const text = JSON.stringify({ ...data, header });
  const digest = host.crypto.md5(`nobody${path}use${text}md5forencrypt`);
  const params = host.crypto.aesEcbEncrypt(KEY, `${path}${SEPARATOR}${text}${SEPARATOR}${digest}`);
  let response;
  try {
    response = await fetch(ORIGIN + path.replace(/^\/api\//, "/eapi/"), {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        "Referer": "https://music.163.com/",
        "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36",
        "Cookie": Object.entries(header).map(([k, v]) => `${k}=${v}`).join("; "),
      },
      body: new URLSearchParams({ params }).toString(),
      signal: AbortSignal.timeout(20000),
    });
  } catch (_) { fail("failed", "无法连接网易云音乐，请检查网络或稍后重试", true); }
  if (!response.ok) {
    if (response.status === 401) fail("signIn", "网易云登录已失效，请重新登录");
    fail("failed", t("网易云服务暂时不可用（HTTP {status}），请稍后重试", { status: response.status }), response.status === 429 || response.status >= 500);
  }
  let result;
  try { result = await response.json(); }
  catch (_) { fail("failed", "网易云返回了无法识别的响应，请稍后重试", true); }
  const code = Number(result.code ?? 200);
  // 扫码的等待、已扫码、过期状态不是通用接口错误。
  if (!options.raw && code !== 200) {
    if ([301, 302, 401].includes(code)) fail("signIn", "网易云登录已失效，请重新登录");
    if (code === -462 || code === 405) fail("failed", "网易云要求安全验证，请先在网易云官方客户端完成验证，再稍后扫码重试");
    fail("failed", t("网易云请求失败（{code}）：{message}", { code, message: String(result.message || result.msg || t("请稍后重试")).slice(0, 180) }));
  }
  const setCookies = response.headers.getSetCookie();
  const cookies = { ...jar };
  for (const line of setCookies) {
    const pair = line.split(";", 1)[0];
    Object.assign(cookies, pairs(pair));
  }
  // 退出登录后，旧的在途请求不能把已清除的会话写回来。
  if (setCookies.length && options.persist !== false && (host.storage.get("sessionEpoch") || 0) === epoch) {
    const current = session();
    for (const line of setCookies) Object.assign(current, pairs(line.split(";", 1)[0]));
    host.storage.set("cookies", current);
  }
  return options.withCookies ? { result, cookies } : result;
}

export async function verify(cookies = session(), checkCurrent = () => {}) {
  const epoch = host.storage.get("sessionEpoch") || 0;
  const { result, cookies: updated } = await api("/api/w/nuser/account/get", {}, {
    cookies, persist: false, withCookies: true,
  });
  const p = result.profile;
  if ((host.storage.get("sessionEpoch") || 0) !== epoch) fail("signIn", "登录状态已变化，请重新扫码");
  checkCurrent();
  if (!p?.userId || !updated.MUSIC_U) fail("signIn", "无法确认网易云账号，请重新登录");
  const account = { id: String(p.userId), name: p.nickname || t("网易云用户"), imageUrl: p.avatarUrl || "" };
  host.storage.set("cookies", updated);
  const old = host.storage.get("account");
  if (old?.id !== account.id) {
    host.storage.remove("likedIds");
    host.storage.remove("likedAt");
  }
  host.storage.set("account", account);
  return account;
}
