import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('all planner and optional setup commands are registered by the Linux host', async()=>{
 const [renderer,planner,host]=await Promise.all([
  readFile(new URL('./index.html',import.meta.url),'utf8'),
  readFile(new URL('./session-planner.mjs',import.meta.url),'utf8'),
  readFile(new URL('../../src-tauri/src/lib.rs',import.meta.url),'utf8')]);
 const relevant=['propose_session_plan','confirm_session_plan','cancel_session_plan','get_local_agent_data','get_desktop_preferences','set_focus_alerts_enabled','set_contextual_focus_alerts_enabled','get_weekly_report_schedule','save_weekly_report_schedule','save_scheduled_report_pdf'];
 for(const command of relevant) {
  assert.ok((renderer+planner).includes(`'${command}'`),`${command} is used by its UI`);
  assert.ok(host.includes(`::${command},`),`${command} is registered by the native host`);
 }
 assert.ok(renderer.includes("await invoke('start_monitoring')"));
 assert.ok(renderer.includes("await invoke('stop_monitoring')"));
});
