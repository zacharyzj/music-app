export const API = "https://api.bilibili.com";
export const SEARCH = "https://search.bilibili.com/";
export const SITE = "https://www.bilibili.com/";
export const PASSPORT = "https://passport.bilibili.com/x/passport-login/web/qrcode/";
export const SESSION_NAMES = ["SESSDATA", "bili_jct", "DedeUserID", "DedeUserID__ckMd5", "sid"];
const VISITOR_NAMES = ["buvid3", "buvid4", "b_nut"];
let visitor;

function checkStatus(code, message) {
  if ([403, 412, 429, -352, -412, -509].includes(code)) {
    throw new Error("哔哩哔哩暂时限制了请求，请稍后重试或在官方客户端完成验证");
  }
  if (code !== 0) throw new Error("哔哩哔哩请求失败（" + code + "）" + (message ? "：" + message : ""));
}

export function issuedCookies(headers, names) {
  const cookies = {};
  for (const line of headers.getSetCookie()) {
    const pair = line.split(";", 1)[0];
    const i = pair.indexOf("=");
    if (i > 0 && names.includes(pair.slice(0, i)) && pair.slice(i + 1) && !/[\s;\x00-\x1f\x7f]/.test(pair.slice(i + 1))) {
      cookies[pair.slice(0, i)] = pair.slice(i + 1);
    }
  }
  return cookies;
}

async function request(url, cookies, referrer) {
  const headers = { "User-Agent": "music-app (Bilibili plugin/1.0)", Referer: referrer };
  const pairs = Object.entries(cookies || {}).map(([key, value]) => key + "=" + value);
  if (pairs.length) headers.Cookie = pairs.join("; ");
  const response = await fetch(url, {headers});
  if (!response.ok) checkStatus(response.status);
  return response;
}

export async function jsonRequest(url, {cookies = {}, referrer = SITE, allowGuest = false} = {}) {
  const response = await request(url, cookies, referrer);
  const body = await response.json();
  if (allowGuest && body.code === -101) return {data: null};
  checkStatus(body.code, body.message);
  return {data: body.data, headers: response.headers};
}

export function session() {
  return host.storage.get("session") || {};
}

export async function searchPage(query, page) {
  // The official search page issues the visitor cookies used by its search API.
  if (!visitor) {
    const entry = await request(SEARCH + "all?" + new URLSearchParams({keyword:query}), {}, SEARCH);
    visitor = issuedCookies(entry.headers, VISITOR_NAMES);
  }
  const params = new URLSearchParams({search_type:"video", keyword:query, page});
  try {
    return (await jsonRequest(API + "/x/web-interface/search/type?" + params, {
      cookies:{...visitor, ...session()}, referrer:SEARCH,
    })).data;
  } catch (error) {
    visitor = null;
    throw error;
  }
}
