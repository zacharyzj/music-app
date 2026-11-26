import { api, fail, id, hasSession, requireAccount, resetSession, verify } from "./client.js";
import * as model from "./models.js";
import { t } from "./i18n.js";

const PAGE = 50;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const offsetOf = (value) => Math.max(0, Math.floor(Number(value) || 0));
const currentId = () => String(host.storage.get("account")?.id || "");
let loginSequence = 0;
let likedPageOrder;

export async function account() {
  if (!hasSession()) return null;
  try { return await verify(); }
  catch (e) {
    // 网络错误保留会话，只有确实过期时才清掉。
    if (e.code !== "signIn") throw e;
    resetSession();
    return null;
  }
}

export async function signIn() {
  // 生成新二维码时保留原会话；用户取消或网络失败不会登出已有账号。
  const original = host.storage.get("cookies");
  const originalAccount = host.storage.get("account");
  const epoch = host.storage.get("sessionEpoch") || 0;
  const sequence = ++loginSequence;
  const deadline = Date.now() + 8 * 60 * 1000;
  let succeeded = false;
  function current() {
    if (sequence !== loginSequence || (host.storage.get("sessionEpoch") || 0) !== epoch) {
      fail("signIn", "扫码登录已取消，请重新点击登录");
    }
    if (Date.now() >= deadline) fail("signIn", "扫码登录超时，请重新点击登录");
  }
  // 取新码或检查状态的短暂网络失败不能直接结束整个登录流程。
  async function request(path, data, options, qr = "") {
    for (let retry = 0; ; retry++) {
      current();
      try {
        const result = await api(path, data, options);
        current();
        return result;
      } catch (e) {
        current();
        if (!e.retryable || retry >= 2) throw e;
        host.signInPrompt({ qr, text: t("网络暂时不稳定，正在重试…") });
        await sleep(1500 * (retry + 1));
      }
    }
  }
  try {
    // 不限制只能刷新三次；在宿主十分钟时限前主动结束。
    while (Date.now() < deadline) {
      current();
      host.signInPrompt({ text: t("正在获取新的登录二维码…") });
      const keyResult = await request("/api/login/qrcode/unikey", { type: 3 }, { cookies: {}, persist: false });
      const key = keyResult.unikey;
      if (!key) fail("failed", "网易云未返回登录二维码，请稍后重试");
      const qr = `https://music.163.com/login?codekey=${encodeURIComponent(key)}`;
      const created = Date.now();
      host.signInPrompt({ qr, text: t("请用网易云音乐 App 扫码，并在手机上确认登录") });
      // 即使服务端一直返回等待状态，也提前轮换，避免展示已失效的旧码。
      // 次数限制还可应对系统时间被用户向后调整的情况。
      for (let poll = 0; poll < 60 && Date.now() - created < 120000; poll++) {
        await sleep(1800);
        current();
        if (Date.now() - created >= 120000) break;
        const { result, cookies } = await request("/api/login/qrcode/client/login", { key, type: 3 }, {
          cookies: {}, raw: true, persist: false, withCookies: true,
        }, qr);
        switch (Number(result.code)) {
          case 800:
            host.signInPrompt({ text: t("二维码已过期，正在刷新…") });
            poll = 60;
            break;
          case 801:
            host.signInPrompt({ qr, text: t("请用网易云音乐 App 扫码，并在手机上确认登录") });
            break;
          case 802:
            host.signInPrompt({ qr, text: t("已扫码，请在手机上确认登录") });
            break;
          case 803: {
            const result = await verify(cookies, current);
            current();
            succeeded = true;
            return result;
          }
          default:
            fail("failed", "网易云暂时无法完成扫码登录，请先在官方客户端完成安全验证后重试");
        }
      }
    }
    fail("signIn", "扫码登录超时，请重新点击登录");
  } finally {
    // 重新点登录后，旧调用的结果不能清掉新调用的二维码或覆盖其会话。
    if (sequence === loginSequence) host.signInPrompt();
    if (!succeeded && sequence === loginSequence && (host.storage.get("sessionEpoch") || 0) === epoch) {
      host.storage.set("cookies", original ?? null);
      host.storage.set("account", originalAccount ?? null);
    }
  }
}

export function signOut() {
  // 仅退出本插件，避免使用户官方客户端的登录失效。
  loginSequence++;
  likedPageOrder = undefined;
  resetSession();
}

export async function search({ query = "", kind = "tracks", offset = 0 }) {
  if (!query.trim()) return { [kind]: [], more: false };
  const types = { tracks: 1, albums: 10, artists: 100, playlists: 1000 };
  const names = { tracks: "songs", albums: "albums", artists: "artists", playlists: "playlists" };
  const counts = { tracks: "songCount", albums: "albumCount", artists: "artistCount", playlists: "playlistCount" };
  const mappers = { tracks: model.track, albums: model.album, artists: model.artist, playlists: (x) => model.playlist(x, currentId()) };
  if (!types[kind]) fail("unsupported", "不支持这种搜索类型");
  const start = offsetOf(offset);
  const j = await api("/api/cloudsearch/pc", { s: query.trim(), type: types[kind], limit: PAGE, offset: start, total: true });
  const r = j.result || {};
  const items = r[names[kind]] || [];
  // 不把 Array.map 的序号误传成 track 的 privilege 参数。
  const mapped = items.map((x) => mappers[kind](x));
  const total = r[counts[kind]];
  return { [kind]: mapped, more: typeof total === "number" ? start + items.length < total : Boolean(r.hasMore), ...(typeof total === "number" ? { total } : {}) };
}

export async function suggest({ query = "" }) {
  if (!query.trim()) return [];
  const j = await api("/api/search/suggest/web", { s: query.trim() });
  return [...new Set([
    ...(j.result?.songs || []).map((x) => x.name),
    ...(j.result?.artists || []).map((x) => x.name),
    ...(j.result?.albums || []).map((x) => x.name),
  ].filter(Boolean))].slice(0, 10);
}

async function details(ids) {
  if (!ids.length) return [];
  const result = [];
  for (let i = 0; i < ids.length; i += 500) {
    const batch = ids.slice(i, i + 500).map(id);
    const j = await api("/api/v3/song/detail", { c: JSON.stringify(batch.map((value) => ({ id: value }))) });
    const privileges = new Map((j.privileges || []).map((x) => [String(x.id), x]));
    const songs = new Map((j.songs || []).map((x) => [String(x.id), model.track(x, privileges.get(String(x.id)))]));
    for (const value of batch) {
      result.push(songs.get(value) || { id: value, name: t("歌曲暂不可用"), unavailable: true });
    }
  }
  return result;
}

export async function album({ id: value }) {
  const j = await api(`/api/v1/album/${id(value)}`);
  const privileges = new Map((j.privileges || []).map((x) => [String(x.id), x]));
  const album = model.album(j.album);
  return {
    album,
    tracks: (j.songs || []).map((x) => ({ ...model.track(x, privileges.get(String(x.id))), album })),
  };
}

export async function artist({ id: value }) {
  const artistId = id(value);
  const [j, albums] = await Promise.all([
    api(`/api/v1/artist/${artistId}`),
    api(`/api/artist/albums/${artistId}`, { limit: 100, offset: 0, total: true }),
  ]);
  return { artist: model.artist(j.artist), tracks: (j.hotSongs || []).map((x) => model.track(x)), albums: (albums.hotAlbums || []).map(model.album), related: [] };
}

function playlistTrackIds(playlist, byAddedTime) {
  const entries = playlist.trackIds || [];
  if (byAddedTime) {
    // at 是歌曲加入这张歌单的时间。稳定排序保留同一时间及缺少时间的条目顺序。
    const addedTime = (entry) => Math.max(0, Number(entry.at) || 0);
    entries.sort((a, b) => addedTime(b) - addedTime(a));
  }
  return entries.map((entry) => String(entry.id));
}

export async function playlist({ id: value }) {
  const j = await api("/api/v6/playlist/detail", { id: id(value), n: 1000, s: 0 });
  if (!j.playlist) fail("unavailable", "歌单不存在，或没有访问权限");
  const p = j.playlist;
  const info = model.playlist(p, currentId());
  const ids = playlistTrackIds(p, info.fixed);
  if (ids.length > 10000) fail("unsupported", "该歌单超过一万首，暂不支持一次载入，请拆分后再打开");
  const privileges = new Map((j.privileges || []).map((x) => [String(x.id), x]));
  const known = new Map((p.tracks || []).map((x) => [String(x.id), model.track(x, privileges.get(String(x.id)))]));
  const missing = ids.filter((x) => !known.has(x));
  for (const t of await details(missing)) known.set(t.id, t);
  const tracks = ids.length ? ids.map((x) => known.get(x)) : [...known.values()];
  return { ...info, tracks, trackCount: p.trackCount ?? tracks.length, heartbeatPlaylistId: info.fixed ? info.id : undefined };
}

async function* userPlaylistPages(uid) {
  // API 最多 1000 条一页；按 more 取完，不能静默丢失后面的歌单。
  for (let offset = 0; offset < 10000; offset += 1000) {
    const j = await api("/api/user/playlist", { uid, limit: 1000, offset, includeVideo: false });
    const items = j.playlist || [];
    yield items.map((x) => model.playlist(x, uid));
    if (!j.more || !items.length) return;
  }
  fail("unsupported", "账号歌单数量超过插件当前支持范围");
}

export async function playlists() {
  const result = [];
  for await (const page of userPlaylistPages(String(requireAccount().id))) result.push(...page);
  return result;
}

async function likedIds() {
  const user = requireAccount();
  const cached = host.storage.get("likedIds");
  if (Array.isArray(cached) && Date.now() - (host.storage.get("likedAt") || 0) < 30000) return cached;
  const j = await api("/api/song/like/get", { uid: user.id });
  const ids = (j.ids || []).map(String);
  host.storage.set("likedIds", ids);
  host.storage.set("likedAt", Date.now());
  return ids;
}

async function readLikedOrder(uid) {
  for await (const page of userPlaylistPages(uid)) {
    const favorite = page.find((p) => p.fixed);
    if (!favorite) continue;
    // trackIds 包含全歌单，歌曲详情仅按当前页取。
    const { playlist } = await api("/api/v6/playlist/detail", { id: id(favorite.id), n: 1, s: 0 });
    if (!Array.isArray(playlist?.trackIds)) return { playlistId: favorite.id };
    const ids = playlistTrackIds(playlist, true);
    if (ids.length >= (playlist.trackCount ?? favorite.trackCount)) return { playlistId: favorite.id, ids };
    if (!ids.length) return { playlistId: favorite.id };
    return { playlistId: favorite.id, ids: [...new Set([...ids, ...await likedIds()])] };
  }
  return null;
}

async function likedOrder(refresh = false) {
  const uid = String(requireAccount().id);
  if (refresh || likedPageOrder?.uid !== uid) {
    let ordered;
    try { ordered = await readLikedOrder(uid); }
    catch (e) {
      if (!["failed", "unavailable", "unsupported"].includes(e.code)) throw e;
    }
    likedPageOrder = {
      uid, playlistId: ordered?.playlistId, ids: ordered?.ids ?? await likedIds(),
      title: ordered?.ids ? t("喜欢的歌曲") : t("喜欢的歌曲 · 网易云返回顺序"),
    };
  }
  return likedPageOrder;
}

export async function liked({ offset = 0 }) {
  const start = offsetOf(offset);
  const { ids, title, playlistId } = await likedOrder(!start);
  return { title, tracks: await details(ids.slice(start, start + PAGE)), more: start + PAGE < ids.length, heartbeatPlaylistId: playlistId };
}

export async function heartbeat({ playlistId, currentId, recent = [] }) {
  const order = await likedOrder();
  if (!order.playlistId || order.playlistId !== playlistId) fail("unsupported", "心动模式只适用于当前账号喜欢的歌曲");
  const liked = new Set(order.ids);
  if (!liked.size) return { tracks: [] };
  const played = new Set(recent.slice(-40).map(String));
  const current = id(currentId);
  played.add(current);
  const fresh = [...liked].filter(song => !played.has(song));
  const seeds = fresh.length ? fresh : order.ids;
  const seed = liked.has(current) ? current : seeds[Math.floor(Math.random() * seeds.length)];
  const response = await api("/api/playmode/intelligence/list", {
    songId: seed, type: "fromPlayOne", playlistId, startMusicId: current, count: 10,
  });

  const picks = [];
  const known = new Map();
  for (const entry of response.data || []) {
    const song = String(entry?.songInfo?.id ?? entry?.id ?? "");
    if (!/^[1-9]\d*$/.test(song) || played.has(song) || liked.has(song)) continue;
    if (entry.songInfo?.name) {
      const track = model.track(entry.songInfo);
      if (track.unavailable) continue;
      known.set(song, track);
    }
    played.add(song);
    picks.push(song);
    if (picks.length === 5) break;
  }

  // Sample across the whole favorite playlist, including pages the user has not opened.
  const pool = !fresh.length && picks.length ? [...liked] : fresh;
  const favorites = [];
  while (pool.length && favorites.length < 5) {
    const i = Math.floor(Math.random() * pool.length);
    favorites.push(pool[i]);
    pool[i] = pool[pool.length - 1];
    pool.pop();
  }
  const mixed = [];
  for (let i = 0; i < Math.max(favorites.length, picks.length); i++) {
    if (picks[i]) mixed.push(picks[i]);
    if (favorites[i]) mixed.push(favorites[i]);
  }
  for (const track of await details(mixed.filter(song => !known.has(song)))) known.set(track.id, track);
  const tracks = mixed.map(song => known.get(song)).filter(track => track && !track.unavailable);
  if (tracks.length && tracks[0].id === (recent[recent.length - 1] || current)) tracks.shift();
  return { tracks: tracks.map(track => ({ ...track, heartbeatRecommendation: !liked.has(track.id) })) };
}

export async function isLiked({ ids = [] }) {
  if (!hasSession()) return Object.fromEntries(ids.map((x) => [x, false]));
  const set = new Set(await likedIds());
  return Object.fromEntries(ids.map((x) => [x, set.has(String(x))]));
}

export async function like({ id: value, on }) {
  requireAccount();
  await api("/api/radio/like", { alg: "itembased", trackId: id(value), like: Boolean(on), time: "3" });
  likedPageOrder = undefined;
  host.storage.remove("likedIds");
  host.storage.remove("likedAt");
}

export async function savedAlbums({ offset = 0 }) {
  requireAccount();
  const j = await api("/api/album/sublist", { limit: PAGE, offset: offsetOf(offset), total: true });
  // 音乐库的原始数据读取路径要求数组；返回空数组时结束分页。
  return (j.data || []).map((x) => ({ ...model.album(x), saved: true }));
}

export async function saveAlbum({ id: value, on }) {
  requireAccount();
  await api(`/api/album/${on ? "sub" : "unsub"}`, { id: id(value) });
}

export async function recent() {
  requireAccount();
  const j = await api("/api/play-record/song/list", { limit: 50 });
  return (j.data?.list || []).filter((x) => x.data?.id).map((x) => model.track(x.data));
}

export async function home() {
  const j = await api("/api/personalized/playlist", { limit: 24, total: true, n: 1000 });
  const sections = [];
  if (hasSession()) sections.push({ title: t("每日推荐"), links: [{ title: t("每日推荐歌曲"), page: "daily", subtitle: t("根据你的听歌喜好更新") }] });
  sections.push({ title: t("推荐歌单"), style: "shelf", playlists: (j.result || []).map((x) => model.playlist(x, currentId())) });
  sections.push({ title: t("发现音乐"), links: [{ title: t("排行榜"), page: "charts" }, { title: t("最新专辑"), page: "new-albums" }, { title: t("热门歌单"), page: "playlists:全部:0" }] });
  return { title: t("网易云音乐"), sections };
}

export async function browse({ page = "" }) {
  if (page === "cloud" || page.startsWith("cloud:")) {
    requireAccount();
    const rawOffset = page === "cloud" ? "0" : page.slice(6);
    if (!/^\d+$/.test(rawOffset) || !Number.isSafeInteger(Number(rawOffset))) fail("unsupported", "不支持这个发现页面");
    const offset = Number(rawOffset);
    const j = await api("/api/v1/cloud/get", { limit: PAGE, offset });
    if (!Array.isArray(j.data)) fail("failed", "网易云返回了无法识别的响应，请稍后重试");
    const next = offset + j.data.length;
    const more = j.data.length > 0 && (j.hasMore == null ? Number(j.count) > next : j.hasMore === true || j.hasMore === 1);
    const sections = [{ title: t("我的云盘"), tracks: j.data.map((x) => model.cloudTrack(x)), empty: t("云盘中还没有歌曲") }];
    const links = [];
    if (offset > 0) links.push({ title: t("上一页"), page: `cloud:${Math.max(0, offset - PAGE)}` });
    if (more) links.push({ title: t("下一页"), page: `cloud:${next}` });
    if (links.length) sections.push({ links });
    return {
      title: t("我的云盘"), sections,
      tabs: [{ title: t("热门歌单"), page: "" }, { title: t("我的云盘"), page: "cloud", selected: true }],
      // 新版宿主的发现页通过 filters 翻页，保留 links 兼容旧版页面。
      filters: links.length ? [links] : [],
    };
  }
  if (page === "daily") {
    requireAccount();
    const j = await api("/api/v3/discovery/recommend/songs", {});
    return { title: t("每日推荐歌曲"), sections: [{ tracks: (j.data?.dailySongs || []).map((x) => model.track(x)) }] };
  }
  if (page === "charts") {
    const j = await api("/api/toplist");
    return { title: t("排行榜"), sections: [{ style: "grid", playlists: (j.list || []).map((x) => model.playlist(x, currentId())) }] };
  }
  if (page === "new-albums") {
    const j = await api("/api/discovery/newAlbum");
    return { title: t("最新专辑"), sections: [{ style: "grid", albums: (j.albums || []).map(model.album) }] };
  }
  const [prefix, category = "全部", offset = "0"] = page.split(":");
  if (page && prefix !== "playlists") fail("unsupported", "不支持这个发现页面");
  const start = offsetOf(offset);
  const j = await api("/api/playlist/list", { cat: category, order: "hot", limit: 30, offset: start, total: true });
  const sections = [{ style: "grid", playlists: (j.playlists || []).map((x) => model.playlist(x, currentId())) }];
  if (j.more) sections.push({ links: [{ title: t("下一页"), page: `playlists:${category}:${start + 30}` }] });
  return {
    title: t("热门歌单"), sections,
    tabs: [...["全部", "华语", "欧美", "日语", "韩语", "轻音乐", "古典", "电子"].map((title) => ({ title: t(title), page: `playlists:${title}:0`, selected: title === category })), { title: t("我的云盘"), page: "cloud" }],
  };
}

function qualityLevels(quality) {
  const levels = { hires: "hires", lossless: "lossless", high: "exhigh", standard: "standard" };
  const order = Object.keys(levels);
  const setting = typeof host.setting === "function" ? host.setting("quality") : "";
  const selected = order.includes(setting) ? setting : order.includes(quality) ? quality : "high";
  return order.slice(order.indexOf(selected)).map(candidate => levels[candidate]);
}

export async function stream({ id: value, quality = "high" }) {
  const songId = id(value);
  for (const level of qualityLevels(quality)) {
    const j = await api("/api/song/enhance/player/url/v1", { ids: JSON.stringify([songId]), level, encodeType: "flac" });
    const s = j.data?.[0];
    // 无完整音频时只向下尝试；登录失效、网络和验证错误由 api 原样报告。
    if (!s?.url || (s.freeTrialInfo && s.freeTrialInfo !== "null")) continue;
    if (!/^https?:\/\//i.test(s.url)) fail("unavailable", "网易云未返回有效的音频地址");
    const loudness = {};
    for (const [gain, peak] of [["gain", "peak"], ["albumGain", "albumPeak"]]) {
      if (Number.isFinite(s[gain]) && (s[gain] !== 0 || s[peak] > 0)) {
        loudness[gain] = s[gain];
        if (Number.isFinite(s[peak]) && s[peak] > 0) loudness[peak] = s[peak];
      }
    }
    return { ...loudness, url: s.url.replace(/^http:\/\//i, "https://"), type: s.type || "mp3", quality: s.level || level, bitRate: s.br || 0, size: s.size || 0 };
  }
  fail("unavailable", hasSession() ? "此歌曲需要会员、单独购买，或当前地区无版权；不会以试听片段代替完整歌曲" : "此歌曲需要登录或相应会员权限，请先扫码登录网易云音乐");
}

export async function download({ id: value, quality = "high" }) {
  requireAccount();
  const songId = id(value);
  const [level] = qualityLevels(quality);
  const j = await api("/api/song/enhance/download/url/v1", { id: songId, level, immerseType: "c51" });
  const file = Array.isArray(j.data) ? j.data[0] : j.data;
  if (!file?.url || (file.code != null && Number(file.code) !== 200) ||
      (file.id != null && String(file.id) !== String(songId)) ||
      (file.freeTrialInfo && file.freeTrialInfo !== "null") || !/^https?:\/\//i.test(file.url)) {
    fail("unavailable", "当前网易云账号没有这首歌所选音质的完整下载权限");
  }
  return { url: file.url.replace(/^http:\/\//i, "https://"), size: Number(file.size) || 0 };
}

export async function lyrics({ id: value }) {
  const j = await api("/api/song/lyric", { id: id(value), tv: -1, lv: -1, rv: -1, kv: -1, _nmclfl: 1 });
  return { lrc: j.lrc?.lyric || "", translation: j.tlyric?.lyric || "" };
}

export function trackMenu() {
  return [{ id: "open-netease", title: t("在网易云音乐网页打开") }];
}

export function menuAction({ id: action, track }) {
  if (action !== "open-netease") fail("unsupported", "未知菜单操作");
  host.openURL(`https://music.163.com/song?id=${id(track.id)}`);
  return {};
}
