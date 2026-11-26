// Browser-like capabilities used by YPM source modules. All I/O crosses __native.
const callHost = (op, args = {}) => {
  const r = JSON.parse(__native(JSON.stringify({op, ...args})));
  if (r.error) throw Object.assign(new Error(r.error), {code:r.code || 'host'});
  return r.value;
};
globalThis.console = Object.freeze({log(){},info(){},warn(){},error(){},debug(){}});
globalThis.host = Object.freeze({
  storage:Object.freeze({get:key=>callHost('get',{key}),set:(key,value)=>callHost('set',{key,value:value??null}),remove:key=>callHost('remove',{key})}),
  crypto:Object.freeze({md5:text=>callHost('md5',{text:String(text)}),aesEcbEncrypt:(key,text)=>callHost('aes',{key,text})}),
  mediaAllowed:url=>callHost('mediaAllowed',{url:String(url)}),
  setting:key=>callHost('setting',{key}), language:()=> 'zh-CN', t:text=>String(text),
  signInPrompt:prompt=>callHost('prompt',{prompt:prompt??null}),
  openURL:()=>{throw new Error('此宿主尚未开放外部链接操作')},
});
globalThis.URLSearchParams = class {
  constructor(value={}) {this.entries=[]; if(typeof value==='string') {for(const part of value.split('&').filter(Boolean)){const i=part.indexOf('=');this.append(decodeURIComponent((i<0?part:part.slice(0,i)).replace(/\+/g,' ')),decodeURIComponent((i<0?'':part.slice(i+1)).replace(/\+/g,' ')))}} else for(const [k,v] of (Array.isArray(value)?value:Object.entries(value))) this.append(k,v);}
  append(k,v){this.entries.push([String(k),String(v)])}
  set(k,v){this.entries=this.entries.filter(p=>p[0]!==k);this.append(k,v)}
  toString(){const enc=s=>encodeURIComponent(s).replace(/%20/g,'+').replace(/[!'()~]/g,c=>'%'+c.charCodeAt(0).toString(16).toUpperCase());return this.entries.map(([k,v])=>`${enc(k)}=${enc(v)}`).join('&')}
};
globalThis.AbortSignal = class { constructor(){this.aborted=false;this.deadline=0} static timeout(ms){const s=new AbortSignal();s.deadline=Date.now()+ms;return s} };
globalThis.AbortController = class {constructor(){this.signal=new AbortSignal()}abort(){this.signal.aborted=true}};
globalThis.Headers = class {
  constructor(headers={},cookies=[]){this.values=Object.fromEntries(Object.entries(headers).map(([k,v])=>[k.toLowerCase(),String(v)]));this.cookies=cookies}
  get(name){return this.values[name.toLowerCase()]??null} getSetCookie(){return [...this.cookies]}
};
globalThis.fetch = async (url, options={}) => {
  if(options.signal?.aborted) throw new Error('请求已取消');
  const timeout=options.signal?.deadline ? Math.max(1,options.signal.deadline-Date.now()):20000;
  const r=callHost('fetch',{url:String(url),method:options.method||'GET',headers:options.headers||{},body:options.body??null,timeout});
  return {ok:r.status>=200&&r.status<300,status:r.status,headers:new Headers(r.headers,r.cookies),text:async()=>r.body,json:async()=>JSON.parse(r.body)};
};
const timers=new Map();let timerId=0;
globalThis.setTimeout=(fn,ms=0,...args)=>{if(timers.size>=1024)throw new Error('定时器过多');const id=++timerId;timers.set(id,{at:Date.now()+Math.max(0,Math.min(Number(ms)||0,600000)),fn:()=>fn(...args)});return id};
globalThis.clearTimeout=id=>timers.delete(id);
globalThis.setInterval=(fn,ms=0,...args)=>{const delay=Math.max(10,Math.min(Number(ms)||0,600000));const id=setTimeout(fn,delay,...args);timers.get(id).interval=delay;return id};
globalThis.clearInterval=clearTimeout;
globalThis.__tick=()=>{const now=Date.now();for(const [id,t] of [...timers])if(t.at<=now&&timers.get(id)===t){if(t.interval)t.at=now+t.interval;else timers.delete(id);t.fn()}};
globalThis.__results=Object.create(null);

const coverUrl=x=>typeof x==='string'&&/^https:\/\//i.test(x)?x:'';
const item=(x,kind,source)=>({key:{source,id:String(x.id??'')},kind,title:x.name||'',search_aliases:Array.isArray(x.searchAliases)?x.searchAliases.filter(s=>typeof s==='string'):undefined,license:x.license||'',subtitle:(x.artists||[]).map(a=>a.name).join(' / ')||x.owner||'',cover:coverUrl(x.imageUrl||x.album?.imageUrl),duration_ms:x.duration||0,unavailable:!!x.unavailable,downloadable:typeof x.downloadable==='boolean'?x.downloadable:undefined,album:x.album?.name||'',artist:(x.artists||[]).map(a=>a.name).join(' / ')});
const plural={track:'tracks',album:'albums',artist:'artists',playlist:'playlists'};
globalThis.__dispatch=async(op,args,source)=>{
  const p=globalThis.__plugin;
  let a={...args};
  if(op==='search') a.kind=plural[a.kind]||a.kind||'tracks';
  if(op==='browse'&&!a.page&&a.id)a.page=a.id;
  if(typeof p[op]!=='function')throw Object.assign(new Error('插件不支持 '+op),{code:'unsupported'});
  const v=await p[op](a);
  if(['account','signIn','signOut','stream','download','lyrics','suggest','like','saveAlbum','isLiked','menuAction','trackMenu'].includes(op))return v??null;
  let items=[],links=[],title=v?.title||({search:'搜索结果',liked:'喜欢的歌曲',playlists:'我的歌单',savedAlbums:'收藏专辑',recent:'最近播放'}[op]||'音乐');
  const add=container=>{for(const [kind,key]of Object.entries(plural))for(const x of(container?.[key]||[]))items.push(item(x,kind,source));for(const x of(container?.links||[]))links.push({title:x.title||'',op:'browse',args:{page:x.page||''}})};
  let more=!!v?.more,offset=args.offset||0;
  if(Array.isArray(v)){
    const kind=op==='playlists'?'playlist':op==='savedAlbums'?'album':'track';
    let values=v;
    if(op==='playlists'){values=v.slice(offset,offset+50);more=offset+50<v.length}
    if(op==='savedAlbums')more=v.length>=50;
    items=values.map(x=>item(x,kind,source));
  }else if(op==='heartbeat'){
    items=(v?.tracks||[]).map(t=>({...item(t,'track',source),heartbeat_recommendation:!!t.heartbeatRecommendation}));
  }else if(v?.sections){for(const s of v.sections)add(s);offset=0;}
  else {add(v);if(op==='album')title=v.album?.name||title;else if(op==='artist')title=v.artist?.name||title;else if(op==='playlist')title=v.name||title;}
  const heartbeatPlaylistId=(op==='liked'||op==='playlist') ? v?.heartbeatPlaylistId : undefined;
  return {title,items,links,more,offset,heartbeatPlaylistId};
};
globalThis.__start=(id,op,args,source)=>{
  __results[id]={pending:true};
  Promise.resolve().then(()=>__dispatch(op,args,source)).then(value=>{__results[id]={value}},e=>{__results[id]={error:String(e?.message||e),code:String(e?.code||'failed')}});
};
