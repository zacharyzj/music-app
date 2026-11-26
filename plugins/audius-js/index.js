import { toTraditional, toSimplified } from "./variants.js";

const API = "https://api.audius.co/v1/tracks";
const PAGE = 50;
let lastSearch;

async function data(url) {
  const response = await fetch(url);
  if (response.status === 429) throw new Error("Audius 请求过于频繁，请稍后再试");
  if (response.status === 404) throw new Error("Audius 上找不到这条录音");
  if (!response.ok) throw new Error("Audius 请求失败（HTTP " + response.status + "）");
  return (await response.json()).data;
}

function visible(track) {
  return !!track?.id && !track.is_unlisted && !track.is_delete && track.is_available !== false;
}

function playable(track) {
  return visible(track) && track.is_streamable === true && track.is_stream_gated === false &&
    track.stream_conditions == null && track.access?.stream === true;
}

function downloadable(track) {
  return visible(track) && track.is_downloadable === true && track.is_download_gated === false &&
    track.download_conditions == null && track.access?.download === true;
}

// The API supplies the signed path and eligible replica origins. Keep the path and query intact.
function mediaUrl(media, field = "url") {
  const url = media?.[field];
  const match = typeof url === "string" && /^(https:\/\/[a-z0-9.-]+)(\/[^\s#]*)$/i.exec(url);
  if (!match) return "";
  if (typeof host.mediaAllowed !== "function" || host.mediaAllowed(url)) return url;
  for (const origin of media.mirrors || []) {
    if (/^https:\/\/[a-z0-9.-]+$/i.test(origin) && host.mediaAllowed(origin + match[2])) return origin + match[2];
  }
  return "";
}

function item(track) {
  const artist = track.user?.name || track.user?.handle || "";
  const aliases = new Set();
  for (const text of [track.title || "", artist]) {
    for (const table of [toSimplified, toTraditional]) {
      const spelling = convert(text, table);
      if (spelling !== text) aliases.add(spelling);
    }
  }
  return {
    id: track.id, name: track.title, artists: [{ name: artist }], searchAliases: [...aliases],
    duration: (track.duration || 0) * 1000, imageUrl: mediaUrl(track.artwork, "150x150"),
    license: typeof track.license === "string" ? track.license : "",
    unavailable: !playable(track), downloadable: downloadable(track),
  };
}

async function findTracks(query, offset, downloadsOnly = false) {
  const params = new URLSearchParams({ limit: PAGE + 1, query, offset });
  if (downloadsOnly) params.set("only_downloadable", "true");
  const tracks = await data(API + "/search?" + params);
  if (!Array.isArray(tracks)) throw new Error("Audius 返回了无效的搜索结果");
  return tracks;
}

function page(tracks, filter = visible) {
  return { tracks: tracks.slice(0, PAGE).filter(filter).map(item), more: tracks.length > PAGE };
}

export async function home({ offset = 0 }) {
  return { ...page(await findTracks("Mandopop", offset, true), downloadable), title: "华语音乐 · 开放下载" };
}

function convert(query, table) {
  return Array.from(query, char => table.get(char) || char).join("");
}

export async function search({ query = "", kind = "tracks", offset = 0 }) {
  const original = query.trim();
  if (!original || kind !== "tracks") return { tracks: [], more: false };
  if (offset > 0 && lastSearch?.original === original) {
    return page(await findTracks(lastSearch.matched, offset));
  }
  const spellings = new Set([original, convert(original, toTraditional), convert(original, toSimplified)]);
  for (const spelling of spellings) {
    const tracks = await findTracks(spelling, 0);
    if (tracks.length) {
      lastSearch = { original, matched: spelling };
      return page(offset ? await findTracks(spelling, offset) : tracks);
    }
  }
  return { tracks: [], more: false };
}

async function getTrack(id) {
  if (typeof id !== "string" || !/^[a-z0-9]+$/i.test(id)) throw new Error("无效的 Audius 录音编号");
  const track = await data(API + "/" + id);
  if (!visible(track) || track.id !== id) throw new Error("Audius 上找不到这条公开录音");
  return track;
}

export async function stream({ id }) {
  const track = await getTrack(id);
  if (!playable(track)) throw new Error("这条录音未在 Audius 开放游客完整播放");
  return { url: mediaUrl(track.stream) || API + "/" + id + "/stream" };
}

export async function download({ id }) {
  const track = await getTrack(id);
  if (!downloadable(track)) throw new Error("发布者未在 Audius 开放这条录音的免登录下载");
  const pageUrl = track.permalink?.startsWith("/") ? "https://audius.co" + track.permalink : "";
  return { url: mediaUrl(track.download) || API + "/" + id + "/download",
    attribution: [track.title, track.user?.name, pageUrl, track.license].filter(s => typeof s === "string" && s).join("\n") };
}

export function lyrics() { return { lrc: "" }; }
