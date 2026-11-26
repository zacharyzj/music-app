import { API, PASSPORT, SESSION_NAMES, issuedCookies, jsonRequest, session } from "./client.js";

async function profile(cookies) {
  if (!cookies.SESSDATA) return null;
  const {data} = await jsonRequest(API + "/x/web-interface/nav", {cookies, allowGuest:true});
  if (data === null || data?.isLogin === false) return null;
  if (data?.isLogin !== true || !data.mid) throw new Error("哔哩哔哩未返回有效的账号资料");
  return {id:String(data.mid), name:data.uname || String(data.mid)};
}

export async function account() {
  const saved = session();
  const user = await profile(saved);
  if (!user && saved.SESSDATA) host.storage.remove("session");
  return user;
}

let loginAttempt = 0;
export function signOut() {
  ++loginAttempt;
  host.storage.remove("session");
}

function loginCookies(headers, callback) {
  const cookies = issuedCookies(headers, SESSION_NAMES);
  if (cookies.SESSDATA) return cookies;
  // Some passport responses put the same fields in a first-party callback URL.
  // Extract them locally and verify them with nav before saving anything.
  const match = /^https:\/\/(?:passport\.biligame\.com|passport\.bilibili\.com|www\.bilibili\.com)(?:\/[^?#]*)?\?([^#]*)$/.exec(callback || "");
  if (match) {
    for (const field of match[1].split("&")) {
      const separator = field.indexOf("=");
      const key = field.slice(0, separator);
      if (separator < 1 || !SESSION_NAMES.includes(key)) continue;
      const value = decodeURIComponent(field.slice(separator + 1));
      if (value && !/[\s;\x00-\x1f\x7f]/.test(value)) cookies[key] = value;
    }
  }
  return cookies;
}

export async function signIn() {
  const attempt = ++loginAttempt;
  const deadline = Date.now() + 8 * 60 * 1000;
  const waiting = "请用哔哩哔哩 App 扫码，并在手机上确认登录";
  function checkAttempt() {
    if (attempt !== loginAttempt) throw new Error("扫码登录已取消");
    if (Date.now() >= deadline) throw new Error("扫码登录已超时，请重新登录");
  }
  try {
    while (true) {
      checkAttempt();
      const {data} = await jsonRequest(PASSPORT + "generate");
      const qr = data?.url;
      if (!data?.qrcode_key || !/^https:\/\/(?:account|passport)\.bilibili\.com\//.test(qr || "")) {
        throw new Error("哔哩哔哩未返回有效的登录二维码");
      }
      host.signInPrompt({qr, text:waiting});
      while (true) {
        await new Promise(resolve => setTimeout(resolve, 2000));
        checkAttempt();
        const poll = await jsonRequest(PASSPORT + "poll?" + new URLSearchParams({qrcode_key:data.qrcode_key}));
        const status = poll.data?.code;
        if (status === 86038) break;
        if (status === 86101 || status === 86090) {
          host.signInPrompt({qr, text:status === 86090 ? "已扫码，请在哔哩哔哩 App 中确认登录" : waiting});
          continue;
        }
        if (status !== 0) throw new Error("哔哩哔哩未确认本次登录，请重新扫码");
        const candidate = loginCookies(poll.headers, poll.data.url);
        const user = await profile(candidate);
        checkAttempt();
        if (!user) throw new Error("哔哩哔哩未确认账号，请重新扫码登录");
        host.storage.set("session", candidate);
        return user;
      }
    }
  } finally {
    if (loginAttempt === attempt) host.signInPrompt();
  }
}
