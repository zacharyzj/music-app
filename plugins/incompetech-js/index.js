const ROOT = "https://incompetech.com/music/royalty-free/";
const ARTIST = "Kevin MacLeod";
const LICENSE = "CC BY 4.0";
const LICENSE_URL = "https://creativecommons.org/licenses/by/4.0/";
const PAGE = 50;
let catalog;
let refreshed = 0;

async function pieces() {
  if (catalog && Date.now() - refreshed < 3600000) return catalog;
  const response = await fetch(ROOT + "pieces.json");
  if (!response.ok) throw new Error("开放曲库请求失败 (" + response.status + ")");
  const data = await response.json();
  if (!Array.isArray(data)) throw new Error("开放曲库目录格式错误");
  catalog = data.filter(p => p.title && p.isrc && typeof p.filename === "string" &&
    !/[\\/\x00-\x1f]/.test(p.filename) && /\.mp3$/i.test(p.filename));
  refreshed = Date.now();
  return catalog;
}

function track(piece) {
  const duration = String(piece.length || "").split(":").reduce((n, part) => n * 60 + Number(part), 0);
  return { id: piece.isrc, name: piece.title, artists: [{ name: ARTIST }], duration: (duration || 0) * 1000, license: LICENSE, downloadable: true };
}

function page(list, offset) {
  return { tracks: list.slice(offset, offset + PAGE).map(track), more: offset + PAGE < list.length };
}

export async function home({ offset = 0 }) {
  return { ...page(await pieces(), offset), title: "Incompetech 开放音乐" };
}

export async function search({ query = "", kind = "tracks", offset = 0 }) {
  if (kind !== "tracks" || !query.trim()) return { tracks: [] };
  const terms = query.toLowerCase().trim().split(/\s+/);
  const matches = (await pieces()).filter(p => {
    const fields = [p.title, p.isrc, ARTIST, p.instruments, p.feel].join(" ").toLowerCase();
    return terms.every(term => fields.includes(term));
  });
  return page(matches, offset);
}

export async function download({ id }) {
  const piece = (await pieces()).find(p => p.isrc === id);
  if (!piece) throw new Error("这首作品已从开放曲库下架");
  return { url: ROOT + "mp3-royaltyfree/" + encodeURIComponent(piece.filename),
    attribution: [piece.title, ARTIST + " (incompetech.com)", LICENSE, LICENSE_URL,
      ROOT + "music.html?isrc=" + encodeURIComponent(piece.isrc)].join("\n") };
}

export const stream = download;
export function lyrics() { return { lrc: "" }; }
