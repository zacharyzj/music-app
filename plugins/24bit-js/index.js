const SITE = "https://www.24bit.net";
const banks = [{endpoint:"One", type:"c"}, {endpoint:"Two", type:"b"}];
const PAGE = 30;
let lastPage;

async function request(path, body) {
  const options = {headers:{"User-Agent":"music-app (24bit plugin/1.0)"}};
  if (body) {
    options.method = "POST";
    options.headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(SITE + path, options);
  if (!response.ok) throw new Error("24bit 请求失败（HTTP " + response.status + "）");
  return response;
}

async function api(path, body) {
  const response = await request("/api" + path, body);
  const value = await response.json();
  if (value?.status !== true) throw new Error(value?.result?.message || "24bit 未批准本次请求");
  return value.result;
}

function media(value) {
  if (typeof value !== "string") return "";
  const url=value.replace(/^(?:http:)?\/\//i,"https://");
  return host.mediaAllowed(url) ? url : "";
}

function row(record, type) {
  return {id:type + ":" + record.id, name:record.name, artists:[{name:record.player || ""}],
    album:{name:record.album || ""}, imageUrl:media(record.cover)};
}

export async function search({query = "", kind = "tracks", offset = 0}) {
  query=query.trim();
  if (!query || kind !== "tracks") return {tracks:[],more:false};
  // Each catalog can run out of matches or be temporarily offline independently.
  const pages = offset > 0 && lastPage?.query === query && lastPage.offset === offset
    ? [...lastPage.next] : banks.map(() => 1 + Math.floor(offset/PAGE));
  const tracks=[];
  const errors=[];
  let completed=0;
  for (const [i,bank] of banks.entries()) {
    if (!pages[i]) continue;
    try {
      const entries=await api("/player/searchOnlineMusic"+bank.endpoint,
        {keyword:encodeURIComponent(query),page:pages[i]});
      if (!Array.isArray(entries)) throw new Error("24bit 返回了无效的搜索结果");
      for (const record of entries) {
        if (record?.name && /^[a-zA-Z0-9_-]{1,128}$/.test(String(record.id || ""))) tracks.push(row(record,bank.type));
      }
      pages[i]=entries.length>=PAGE ? pages[i]+1 : 0;
      completed++;
    } catch (error) {
      errors.push(error.message);
    }
  }
  if (!completed && errors.length) throw new Error(errors[0]);
  lastPage={query,offset:offset+PAGE,next:pages};
  return {tracks,more:pages.some(Boolean)};
}

// Read the public player props from the site's streamed Next.js document as JSON.
function playerData(html) {
  let payload="";
  for (const script of html.matchAll(/self\.__next_f\.push\((\[\s*1\s*,\s*"(?:\\.|[^"\\])*"\s*\])\)/g)) {
    payload += JSON.parse(script[1])[1];
  }
  const field=/"itemMusic"\s*:\s*(\{)/.exec(payload);
  if (!field) throw new Error("24bit 暂未提供歌曲详情，请稍后重试");
  const start=field.index+field[0].length-1;
  let depth=0, quoted=false;
  for (let i=start;i<payload.length;i++) {
    const c=payload[i];
    if (quoted && c==="\\") { i++; continue; }
    if (c==='"') { quoted=!quoted; continue; }
    if (quoted) continue;
    if (c==="{") depth++;
    if (c==="}" && --depth===0) return JSON.parse(payload.slice(start,i+1));
  }
  throw new Error("24bit 歌曲详情不完整");
}

async function detail(id) {
  const match=/^([bc]):([A-Za-z0-9_-]{1,128})$/.exec(id || "");
  if (!match) throw new Error("无效的 24bit 歌曲编号");
  const path="/music/"+match[1]+"/"+match[2];
  const html=await (await request(path)).text();
  const item=playerData(html);
  if (String(item.id)!==match[2] || !item.name || !media(item.url)) throw new Error("24bit 当前没有可用的公开音轨");
  return {item, type:match[1], page:SITE+path, url:media(item.url)};
}

export function home() {
  return {title:"24bit 无损音乐",links:[{title:"华语流行",page:"华语流行"},{title:"古典音乐",page:"古典音乐"}]};
}
export async function browse({page}) {
  if (!["华语流行","古典音乐"].includes(page)) throw new Error("没有这个分类");
  const result=await search({query:page});
  return {title:page,tracks:result.tracks};
}
export async function stream({id}) { return {url:(await detail(id)).url}; }
export async function download({id}) {
  const {item,type,page,url}=await detail(id);
  const grant=await api("/music/getOnlineDownload",{type,name:item.name,player:item.player,album:item.album || "未知"});
  if (grant==="limit") throw new Error("24bit 今日游客下载额度已用完，请明日再试");
  if (grant!==true && grant!=="ok") {
    throw new Error("24bit 未确认本次下载额度");
  }
  return {url,attribution:[item.name,item.player,page].filter(Boolean).join("\n")};
}
export function lyrics() { return {lrc:""}; }
