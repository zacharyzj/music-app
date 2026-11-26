import { API, SITE, jsonRequest, searchPage, session } from "./client.js";
export {account, signIn, signOut} from "./account.js";

const PAGE = 20;
const BV = /^BV[0-9A-Za-z]{10}$/;
const entities = {amp:"&", lt:"<", gt:">", quot:'"', apos:"'", nbsp:" "};

function plain(text) {
  return String(text || "").replace(/<\/?em(?:\s+[^>]*)?>/gi, "")
    .replace(/&(amp|lt|gt|quot|apos|nbsp|#\d+|#x[\da-f]+);/gi, (original, code) => {
      if (code[0] !== "#") return entities[code.toLowerCase()] ?? original;
      const n = code[1].toLowerCase() === "x" ? parseInt(code.slice(2), 16) : Number(code.slice(1));
      return n > 0 && n <= 0x10ffff ? String.fromCodePoint(n) : original;
    }).trim();
}

function mediaUrl(raw) {
  if (typeof raw !== "string") return "";
  const url = raw.replace(/^(?:http:)?\/\//i, "https://");
  return host.mediaAllowed(url) ? url : "";
}

function track(video) {
  const seconds = typeof video.duration === "number" ? video.duration :
    String(video.duration || "0").split(":").reduce((total, part) => total * 60 + Number(part), 0);
  return {
    id:video.bvid, name:plain(video.title), artists:[{name:plain(video.author || video.owner?.name)}],
    duration:Number.isFinite(seconds) && seconds > 0 ? Math.round(seconds * 1000) : 0,
    imageUrl:mediaUrl(video.pic), downloadable:false,
  };
}

function tracks(videos) {
  return videos.filter(v => v && BV.test(v.bvid) && v.title).map(track);
}

export async function search({query = "", kind = "tracks", offset = 0}) {
  query = query.trim();
  if (!query || kind !== "tracks") return {tracks:[], more:false};
  const page = 1 + Math.floor(offset / PAGE);
  const data = await searchPage(query, page);
  if (!data || (!Array.isArray(data.result) && data.numResults !== 0)) {
    throw new Error("哔哩哔哩返回了无效的搜索结果");
  }
  return {tracks:tracks(data.result || []), more:page < Number(data.numPages)};
}

const topics = ["音乐现场", "翻唱", "游戏原声"];
export function home() {
  return {title:"哔哩哔哩", links:topics.map(title => ({title, page:title}))};
}

export async function browse({page}) {
  if (!topics.includes(page)) throw new Error("没有这个哔哩哔哩分类");
  const result = await search({query:page});
  return {title:page, tracks:result.tracks};
}

function firstMedia(representations) {
  for (const rep of representations) {
    for (const raw of [rep.baseUrl || rep.base_url || rep.url, ...(rep.backupUrl || rep.backup_url || [])]) {
      const url = mediaUrl(raw);
      if (url) return url;
    }
  }
  return "";
}

// Bilibili's quality IDs describe nominal resolution, including for portrait videos.
const qualityHeight = {6:240, 16:360, 32:480, 64:720, 74:720, 80:1080, 112:1080,
  116:1080, 120:2160, 125:2160, 126:2160, 127:4320};

function videoQualities(tracks) {
  const byHeight = new Map();
  tracks.sort((a, b) => Number(b.codecid === 7) - Number(a.codecid === 7) || b.bandwidth - a.bandwidth);
  for (const rep of tracks) {
    const height = qualityHeight[rep.id] || Math.min(rep.width || rep.height, rep.height);
    if (!Number.isInteger(height) || height <= 0 || byHeight.has(height)) continue;
    const url = firstMedia([rep]);
    if (url) byHeight.set(height, {height, url});
  }
  return [...byHeight.values()].sort((a, b) => b.height - a.height);
}

export async function stream({id, video = false}) {
  if (typeof id !== "string" || !BV.test(id)) throw new Error("无效的哔哩哔哩 BV 号");
  const cookies = session();
  const referrer = SITE + "video/" + id + "/";
  const view = await jsonRequest(API + "/x/web-interface/view?bvid=" + id, {cookies, referrer});
  const cid = view.data?.pages?.[0]?.cid || view.data?.cid;
  if (!cid) throw new Error("这个视频没有可播放的分 P");
  const params = new URLSearchParams({bvid:id, cid, fnval:16});
  if (video) {
    params.set("qn", "127");
    params.set("fourk", "1");
  }
  const {data} = await jsonRequest(API + "/x/player/playurl?" + params, {cookies, referrer});
  if (data?.is_preview) throw new Error("当前账号只获准试看这个视频");
  const audio = [...(data?.dash?.audio || [])].sort((a, b) => b.bandwidth - a.bandwidth);
  const audioUrl = firstMedia(audio);
  const combined = data?.durl?.length === 1 ? firstMedia(data.durl) : "";
  if (video) {
    const quality = Number(data?.quality) || 80;
    const tracks = (data?.dash?.video || []).filter(rep => !rep.id || rep.id <= quality);
    const qualities = videoQualities(tracks);
    const initial = qualities.find(q => q.height <= 1080) || qualities[0];
    // Legacy DASH responses may omit dimensions for the single picture stream.
    const url = initial?.url || firstMedia(tracks);
    if (url && audioUrl) return {url, audioUrl, referrer, qualities};
    if (combined) {
      const height = qualityHeight[data?.quality];
      return {url:combined, referrer, qualities:height ? [{height, url:combined}] : []};
    }
    if (url && !audio.length) return {url, referrer, qualities};
    throw new Error("哔哩哔哩未向当前账号或游客提供可播放的视频");
  }
  if (audioUrl || combined) return {url:audioUrl || combined, referrer};
  throw new Error("哔哩哔哩未向当前账号或游客提供可播放的音轨");
}

export function suggest() { return []; }
export function lyrics() { return {lrc:""}; }
