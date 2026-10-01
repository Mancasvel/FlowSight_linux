// Linux renderer checks with synthetic native responses, never personal data.
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium } from 'playwright';

const output=resolve('.impeccable/review');
await mkdir(output,{recursive:true});
const browser=await chromium.launch({headless:true});
const evidence=[];
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
    get_status:{isRunning:false},check_local_server:{online:false},get_week_summary:{days:[]},get_today_history:{total_seconds:0,entries:[],category_breakdown:[],ticket_breakdown:[]},
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
  for(const selector of ['#onboardingChooseFolder','#onboardingContinueBtn']) {
   const button=page.locator(selector);await button.scrollIntoViewIfNeeded();await page.mouse.move(0,0);await page.waitForTimeout(150);
   const base=await button.evaluate(button=>getComputedStyle(button).backgroundColor);
   await button.hover();await page.waitForTimeout(150);
   const hover=await button.evaluate(button=>getComputedStyle(button).backgroundColor);
   const light=color=>{const c=color.match(/[\d.]+/g).slice(0,3).map(Number);return(Math.max(...c)+Math.min(...c))/510;};
   const delta=light(hover)-light(base);
   assert.ok(delta>=-.01&&delta<=.08,`${selector}: ${base} -> ${hover}, delta=${delta}`);
   evidence.push({viewport,selector,base,hover,lightnessDelta:delta});
  }
  await page.locator('#onboardingContinueBtn').click();
  await page.locator('#onboardingOverlay').waitFor({state:'hidden'});
  assert.equal(await page.locator('#sessionPlanner[open]').count(),1);
  const calls=await page.evaluate(()=>window.testCalls);
  assert.equal(calls.some(call=>['start_monitoring','confirm_session_plan','propose_session_plan'].includes(call.command)),false);
  assert.ok(calls.some(call=>call.command==='set_focus_alerts_enabled'&&call.args.enabled));
  assert.ok(calls.some(call=>call.command==='set_contextual_focus_alerts_enabled'&&call.args.enabled));
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  assert.equal(await page.getByText('Notion',{exact:true}).count(),0);
  assert.deepEqual(errors,[]);
  evidence.push({viewport,blocks,independentConsent:true,noTrackingOrCalendarWrites:true});
  console.log(`${viewport.width}x${viewport.height}: label containment, notification preview, independent consent, subdued hovers, four-step completion passed.`);
  await page.close();
 }
 await writeFile(resolve(output,'linux-session-ui-verification.json'),JSON.stringify(evidence,null,2)+'\n');
} finally { await browser.close(); }
