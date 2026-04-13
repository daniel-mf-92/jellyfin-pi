const puppeteer = require("puppeteer");
const fs = require("fs");
const path = require("path");
const { execSync } = require("child_process");
const OUT = path.join(__dirname, "reference-screenshots");
const JF = "http://localhost:8096";
const TOKEN = "e73bce23994e46a5865e35453e667d9b";
const UID = "dd8fb5c3eec64d33a5381ed503768e09";
const SID = "3848d71b4d7a4e0188f86071b9628847";
fs.mkdirSync(OUT, { recursive: true });
const wait = ms => new Promise(r => setTimeout(r, ms));

(async () => {
  const b = await puppeteer.launch({headless:"new",args:["--no-sandbox"],defaultViewport:{width:1920,height:1080}});
  const p = await b.newPage();

  await p.goto(JF+"/web/index.html", {waitUntil:"networkidle2",timeout:30000});
  await wait(2000);
  await p.screenshot({path:path.join(OUT,"01-login.png")});
  console.log("01-login");

  // Click daniel user
  await p.evaluate(() => { document.querySelectorAll("button").forEach(b => { if(b.textContent.includes("daniel")) b.click(); }); });
  await wait(2000);
  // Type password and submit
  try {
    await p.type("input[type=password]", "5991");
    await wait(500);
    await p.evaluate(() => { document.querySelectorAll("button").forEach(b => { if(b.textContent.trim()==="Sign In"||b.textContent.trim()==="Submit") b.click(); }); });
  } catch(e) { console.log("Password step failed: "+e.message); }
  await wait(6000);

  await p.screenshot({path:path.join(OUT,"02-home.png")});
  const sz = fs.statSync(path.join(OUT,"02-home.png")).size;
  console.log("02-home ("+Math.round(sz/1024)+"KB)");
  const txt = await p.evaluate(() => document.body.innerText.substring(0,200));
  console.log("Page: "+txt.replace(/\n/g," ").substring(0,150));

  // Get library IDs
  let mId="",tId="",cId="";
  try {
    const j=JSON.parse(execSync(`curl -s "${JF}/Users/${UID}/Views?api_key=${TOKEN}"`,{encoding:"utf8"}));
    j.Items.forEach(v=>{
      if(v.CollectionType==="movies")mId=v.Id;
      if(v.CollectionType==="tvshows")tId=v.Id;
      if(v.CollectionType==="boxsets")cId=v.Id;
    });
  } catch(e){}

  const nav = async (url,name,d) => {
    await p.goto(url,{waitUntil:"networkidle2",timeout:30000});
    await wait(d);
    await p.screenshot({path:path.join(OUT,name)});
    console.log(name+" ("+Math.round(fs.statSync(path.join(OUT,name)).size/1024)+"KB)");
  };

  if(mId) await nav(`${JF}/web/index.html#!/list.html?parentId=${mId}&serverId=${SID}`,"03-movies-library.png",4000);
  if(tId) await nav(`${JF}/web/index.html#!/list.html?parentId=${tId}&serverId=${SID}`,"04-tvshows-library.png",4000);

  try {
    const mi=JSON.parse(execSync(`curl -s "${JF}/Users/${UID}/Items?ParentId=${mId}&Limit=1&SortBy=Name&api_key=${TOKEN}"`,{encoding:"utf8"}));
    if(mi.Items.length) await nav(`${JF}/web/index.html#!/details?id=${mi.Items[0].Id}&serverId=${SID}`,"05-movie-detail.png",5000);
  } catch(e){}

  try {
    const si=JSON.parse(execSync(`curl -s "${JF}/Users/${UID}/Items?ParentId=${tId}&Limit=1&IncludeItemTypes=Series&SortBy=Name&api_key=${TOKEN}"`,{encoding:"utf8"}));
    if(si.Items.length) await nav(`${JF}/web/index.html#!/details?id=${si.Items[0].Id}&serverId=${SID}`,"06-tvshow-detail.png",5000);
  } catch(e){}

  await nav(`${JF}/web/index.html#!/search.html`,"07-search.png",2000);
  await nav(`${JF}/web/index.html#!/mypreferencesmenu.html`,"08-settings.png",2000);
  if(cId) await nav(`${JF}/web/index.html#!/list.html?parentId=${cId}&serverId=${SID}`,"09-collections.png",4000);

  await b.close();
  console.log("\nDone. Files:");
  fs.readdirSync(OUT).sort().forEach(f=>console.log("  "+f+" ("+Math.round(fs.statSync(path.join(OUT,f)).size/1024)+"KB)"));
})();
