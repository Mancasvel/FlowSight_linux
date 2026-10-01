// Linux renderer checks with synthetic native responses, never personal data.
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium } from 'playwright';

const output=resolve('.impeccable/review');
await mkdir(output,{recursive:true});
const browser=await chromium.launch({headless:true});
const evidence=[];
const captureBefore=process.argv.includes('--capture-before');
const stage=captureBefore?'before':'after';
const lightness=rgb=>(Math.max(...rgb)+Math.min(...rgb))/510;
const luminance=rgb=>rgb.map(v=>v/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4).reduce((sum,v,i)=>sum+v*[.2126,.7152,.0722][i],0);
const contrast=(a,b)=>{const values=[luminance(a),luminance(b)].sort((a,b)=>b-a);return(values[0]+.05)/(values[1]+.05);};

async function inspectButton(page,selector,name,viewport,{minimumContrast=4.5,syntheticTitlebar=false}={}) {
 const button=page.locator(selector);
 await button.scrollIntoViewIfNeeded();await page.mouse.move(0,0);await button.evaluate(b=>b.blur());await page.waitForTimeout(180);
 const read=()=>button.evaluate(b=>{
  const parse=color=>{const c=color.match(/[\d.]+/g)?.map(Number)||[0,0,0,0];return[c[0],c[1],c[2],c[3]??1];};
  // Resolve translucent fills against every ancestor, rather than treating rgba RGB channels as opaque.
  const layers=[];for(let element=b;element;element=element.parentElement)layers.unshift(getComputedStyle(element).backgroundColor);
  let background=[16,29,37];for(const layer of layers){const c=parse(layer);background=background.map((v,i)=>c[i]*c[3]+v*(1-c[3]));}
  const css=getComputedStyle(b),color=parse(css.color);
  const foreground=background.map((v,i)=>color[i]*color[3]+v*(1-color[3]));
  return{rawBackground:css.backgroundColor,background,foreground,outline:css.outlineStyle,outlineWidth:parseFloat(css.outlineWidth),focusVisible:b.matches(':focus-visible')};
 });
 const base=await read();
 await button.hover();await page.waitForTimeout(180);const hover=await read();
 await page.screenshot({path:resolve(output,`linux-dark-hover-${name}-${viewport.width}x${viewport.height}-${stage}.png`)});
 await page.mouse.move(0,0);await page.keyboard.press('Tab');await button.focus();await page.waitForTimeout(180);const focused=await read();
 await button.evaluate(b=>{b.disabled=true;b.blur();});await page.mouse.move(0,0);await page.waitForTimeout(180);const disabledBase=await read();
 await button.hover({force:true});await page.waitForTimeout(180);const disabledHover=await read();
 await button.evaluate(b=>{b.disabled=false;});
 const delta=lightness(hover.background)-lightness(base.background);
 evidence.push({viewport,selector,name,syntheticTitlebar,base,hover,focused,disabledBase,disabledHover,lightnessDelta:delta,hoverContrast:contrast(hover.foreground,hover.background),focusContrast:contrast(focused.foreground,focused.background)});
 if(!captureBefore){
  assert.ok(delta>=-.005&&delta<=.08,`${name}: hover must slightly lighten its base, delta=${delta}, ${JSON.stringify({base,hover})}`);
  assert.ok(lightness(hover.background)<.4,`${name}: hover stays dark`);
  assert.ok(contrast(hover.foreground,hover.background)>=minimumContrast,`${name}: readable hover contrast ${contrast(hover.foreground,hover.background)}`);
  assert.ok(lightness(focused.background)<.4,`${name}: keyboard focus stays dark`);
  assert.ok(contrast(focused.foreground,focused.background)>=minimumContrast,`${name}: readable focus contrast`);
  assert.ok(focused.focusVisible&&focused.outline==='solid'&&focused.outlineWidth>=2,`${name}: visible keyboard focus outline`);
  assert.equal(disabledHover.rawBackground,disabledBase.rawBackground,`${name}: disabled controls retain their background on hover`);
 }
}
try {
 for(const viewport of [{width:370,height:700},{width:340,height:400},{width:900,height:800}]) {
  const page=await browser.newPage({viewport,colorScheme:'dark',reducedMotion:'reduce',timezoneId:'Europe/Madrid'});
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(()=>{
   window.testCalls=[];let counter=1;
   const prefs={onboardingCompleted:false,displayName:'',workRoles:[],workActivities:[],improvementGoals:[],dailyGoalHours:6};
   const desktop={focusAlertsEnabled:false,contextualFocusAlertsEnabled:false};
   const schedule={enabled:false,weekday:5,time:'17:00',folder:'',revision:0};
   const defaults={initialize_agent:null,get_config:{captureInterval:60000,dailyGoalHours:6},get_auth_session:null,get_current_user:null,
    get_entitlements:{plan:'free',status:'active',can_integrations:false,can_cloud_ai:false,can_sync:false,team_ids:[]},
    get_status:{isRunning:false},check_local_server:{online:false},get_week_summary:{days:[]},get_today_history:{date:'2026-10-01',total_seconds:0,entries:[],category_breakdown:[],ticket_breakdown:[]},
    get_mcp_connection_info:{command:'/Example/FlowSight'},
    get_user_preferences:prefs,get_desktop_preferences:desktop,get_weekly_report_schedule:schedule,get_local_agent_data:{events:[],preferences:{},tasks:[]}};
   window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
   window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{windowLabel:'main',label:'main'}},
    transformCallback(){return counter++;},unregisterCallback(){},convertFileSrc(path){return path;},
    async invoke(command,args={}) {
     window.testCalls.push({command,args});
     if(command==='save_user_preferences_command'){Object.assign(prefs,args.prefs);return structuredClone(prefs);}
     if(command==='set_focus_alerts_enabled'){desktop.focusAlertsEnabled=args.enabled;if(!args.enabled)desktop.contextualFocusAlertsEnabled=false;return args.enabled;}
     if(command==='set_contextual_focus_alerts_enabled'){desktop.contextualFocusAlertsEnabled=args.enabled;return args.enabled;}
     if(command==='save_weekly_report_schedule'){Object.assign(schedule,args.schedule);return structuredClone(schedule);}
     if(command.startsWith('plugin:window|'))return command.endsWith('is_maximized')?false:null;
     if(command==='plugin:event|listen')return args.handler;
     if(command==='get_flowsight_platform')return 'linux';
     if(command.startsWith('plugin:event|'))return null;
     return command in defaults?structuredClone(defaults[command]):null;
    }};
  });
  await page.goto(process.env.FLOWSIGHT_RENDERER_URL||'http://127.0.0.1:1436',{waitUntil:'networkidle'});
  await page.locator('#onboardingOverlay.visible').waitFor();await page.evaluate(()=>document.fonts.ready);
  await page.evaluate(()=>document.body.classList.add('platform-linux'));
  await page.locator('#onboardingContinueBtn').click();
  await page.locator('.onboarding-title').filter({hasText:'A plan you can change'}).waitFor();
  const blocks=await page.locator('.onboarding-demo svg').evaluate(svg=>{
   const rects=[...svg.querySelectorAll('rect')],texts=[...svg.querySelectorAll('text')];
   return texts.map((text,index)=>{const box=text.getBBox(),rect=rects[index];return {label:text.textContent,inside:box.x>=rect.x.baseVal.value&&box.x+box.width<=rect.x.baseVal.value+rect.width.baseVal.value&&box.y>=rect.y.baseVal.value&&box.y+box.height<=rect.y.baseVal.value+rect.height.baseVal.value};});
  });
  assert.ok(blocks.every(block=>block.inside),JSON.stringify(blocks));
  await page.locator('#onboardingOpenPlan').check();
  await page.locator('#onboardingBody').evaluate(body=>{body.scrollTop=0;});
  await page.screenshot({path:resolve(output,`linux-onboarding-plan-${viewport.width}x${viewport.height}.png`)});
  await page.locator('#onboardingContinueBtn').click();
  await page.locator('#onboardingNotificationPreview').waitFor();
  assert.equal(await page.locator('#onboardingContextReminders').isDisabled(),true);
  assert.match(await page.locator('.onboarding-notification-message').textContent(),/Could you choose one task/);
  assert.equal(await page.evaluate(()=>window.testCalls.some(call=>call.command==='set_focus_alerts_enabled')),false);
  await page.locator('#onboardingFocusReminders').check();await page.locator('#onboardingContextReminders').check();
  assert.match(await page.locator('.onboarding-notification-message').textContent(),/Write proposal/);
  await page.locator('#onboardingNotificationPreview').scrollIntoViewIfNeeded();
  await page.screenshot({path:resolve(output,`linux-notification-example-${viewport.width}x${viewport.height}.png`)});
  await page.locator('#onboardingContinueBtn').click();
  await page.locator('#onboardingChooseFolder').waitFor();
  await inspectButton(page,'#onboardingChooseFolder','onboarding-secondary',viewport);
  await inspectButton(page,'#onboardingContinueBtn','onboarding-primary',viewport);
  await page.locator('#onboardingContinueBtn').click();
  await page.locator('#onboardingOverlay').waitFor({state:'hidden'});
  assert.equal(await page.locator('#sessionPlanner[open]').count(),1);
  // Display actual renderer controls in synthetic visual states; no tracking/window command is invoked.
  await page.locator('#stopTimerBtn').evaluate(button=>{button.style.display='flex';button.disabled=false;});
  await inspectButton(page,'#stopTimerBtn','stop',viewport);
  await page.evaluate(()=>document.body.classList.remove('platform-linux'));
  await inspectButton(page,'#winMaximizeBtn','maximize-renderer-preview',viewport,{minimumContrast:3,syntheticTitlebar:true});
  await page.evaluate(()=>document.body.classList.add('platform-linux'));
  await page.locator('#navSummary').click();await page.locator('#generateReportBtn').waitFor();
  await inspectButton(page,'#generateReportBtn','work-report',viewport);
  await page.locator('#navProfile').click();
  await inspectButton(page,'#showMcpConnectionBtn','settings-secondary',viewport);
  await page.locator('#showMcpConnectionBtn').click();
  await inspectButton(page,'#copyMcpCommandBtn','settings-ghost',viewport);
  const calls=await page.evaluate(()=>window.testCalls);
  assert.equal(calls.some(call=>['start_monitoring','confirm_session_plan','propose_session_plan'].includes(call.command)),false);
  assert.ok(calls.some(call=>call.command==='set_focus_alerts_enabled'&&call.args.enabled));
  assert.ok(calls.some(call=>call.command==='set_contextual_focus_alerts_enabled'&&call.args.enabled));
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  assert.equal(await page.getByText('Notion',{exact:true}).count(),0);
  assert.deepEqual(errors,[]);
  evidence.push({viewport,blocks,independentConsent:true,noTrackingOrCalendarWrites:true});
  console.log(`${viewport.width}x${viewport.height}: label containment, notification preview, independent consent, seven dark button hover/focus/disabled states, four-step completion ${captureBefore?'captured':'passed'}.`);
  await page.close();
 }
 await writeFile(resolve(output,`linux-session-ui-verification${captureBefore?'-before':''}.json`),JSON.stringify(evidence,null,2)+'\n');
} finally { await browser.close(); }
