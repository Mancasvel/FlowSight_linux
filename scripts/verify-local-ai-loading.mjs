// Exercises the real desktop renderer with a mocked native host. These are UI
// previews: no model, personal database, or screen capture is involved.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium } from 'playwright';

const output = resolve('.impeccable/review');
await mkdir(output, { recursive: true });

const browser = await chromium.launch({ headless: true });
try {
  for (const { width, height, colorScheme } of [
    { width: 370, height: 700, colorScheme: 'light' },
    { width: 340, height: 400, colorScheme: 'dark' },
  ]) {
    const page = await browser.newPage({
      viewport: { width, height },
      colorScheme,
      reducedMotion: 'reduce',
      locale: 'en-GB',
    });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      const callbacks = new Map();
      const listeners = new Map();
      let callbackId = 0;
      let starts = 0;
      window.modelTest = {
        listeners,
        get starts() { return starts; },
        emit(event, payload) { listeners.get(event)?.({ payload }); },
        fail(message) { this.rejectStart?.(new Error(message)); },
      };
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener(event) { listeners.delete(event); },
      };
      window.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: 'main' },
          currentWebview: { windowLabel: 'main', label: 'main' },
        },
        transformCallback(handler) {
          const id = ++callbackId;
          callbacks.set(id, handler);
          return id;
        },
        unregisterCallback(id) { callbacks.delete(id); },
        convertFileSrc(path) { return path; },
        async invoke(command, args = {}) {
          if (command === 'plugin:event|listen') {
            listeners.set(args.event, callbacks.get(args.handler));
            return args.handler;
          }
          if (command.startsWith('plugin:event|') || command.startsWith('plugin:updater|')) return null;
          if (command.startsWith('plugin:window|')) return command.endsWith('is_maximized') ? false : null;
          if (command === 'plugin:app|version') return '5.0.17';
          if (command === 'local_model_status') return { ready: true };
          if (command === 'start_server') {
            starts++;
            return new Promise((resolveStart, rejectStart) => {
              window.modelTest.resolveStart = resolveStart;
              window.modelTest.rejectStart = rejectStart;
            });
          }
          const defaults = {
            initialize_agent: null,
            get_config: { captureInterval: 60000, dailyGoalHours: 6 },
            get_auth_session: null,
            get_current_user: null,
            get_entitlements: { plan: 'free', status: 'active', can_integrations: false, can_cloud_ai: false, can_sync: false, team_ids: [] },
            get_privacy_settings: { monitoringNoticeAcknowledged: true, noticeVersion: '2026-08-23', cloudSyncEnabled: false, cloudAiEnabled: false, storeWindowTitles: false, excludedApplications: [], retentionDays: 30 },
            get_analytics_consent: { decided: true, consented: false },
            get_status: { isRunning: false },
            check_installation_health: { healthy: true },
            check_local_server: { online: false },
            get_week_summary: { days: [] },
            get_today_history: { total_seconds: 0, entries: [], category_breakdown: [], ticket_breakdown: [], focus: {} },
            get_calendar_companion_status: { googleConnected: false, microsoftConnected: false, googleAvailable: false, microsoftAvailable: false, current: null },
            get_notion_status: { connected: false },
            get_coach_chat_messages: [],
            get_coach_chat_usage: { used: 0 },
            get_browser_pairing: { connected: false },
            get_local_agent_data: { events: [], preferences: {}, tasks: [] },
            get_desktop_preferences: { focusAlertsEnabled: false, contextualFocusAlertsEnabled: false, promptDecided: true },
            get_user_preferences: { onboardingCompleted: true, displayName: '', workRoles: [], workActivities: [], improvementGoals: [], dailyGoalHours: 6 },
            get_weekly_report_schedule: { enabled: false, weekday: 5, time: '17:00', folder: '', revision: 0 },
          };
          return command in defaults ? structuredClone(defaults[command]) : null;
        },
      };
    });
    await page.goto(process.env.FLOWSIGHT_RENDERER_URL || 'http://127.0.0.1:1436', { waitUntil: 'networkidle' });
    await page.evaluate(() => document.fonts.ready);
    await page.locator('#playTimerBtn').waitFor({ state: 'visible' });
    await page.locator('#playTimerBtn').click();
    await page.locator('#setupOverlay').waitFor({ state: 'visible' });
    await page.waitForFunction(() => window.modelTest?.listeners.has('local-ai-startup-progress'));
    await page.evaluate(() => {
      window.modelTest.emit('local-ai-startup-progress', { phase: 'preparing', backend: 'Vulkan GPU', attempt: 1, total: 2 });
      window.modelTest.emit('local-ai-progress', { phase: 'llama-bin', percent: 48, downloaded: 16000000, total: 33000000 });
    });
    await page.locator('#setupStatus').filter({ hasText: 'Downloading AI engine' }).waitFor();
    assert.equal(await page.locator('#setupProgressContainer').isVisible(), true);
    assert.equal(await page.locator('#setupProgress').evaluate(element => element.style.width), '48%');
    await page.evaluate(() => {
      window.modelTest.emit('local-ai-startup-progress', { phase: 'preparing', backend: 'CPU', attempt: 2, total: 2 });
      window.modelTest.emit('local-ai-startup-progress', { phase: 'loading', backend: 'CPU', attempt: 2, total: 2 });
    });
    await page.locator('#setupStatus').filter({ hasText: 'Loading the local model' }).waitFor();
    assert.equal(await page.locator('#setupProgressContainer').isVisible(), false);
    await page.screenshot({ path: resolve(output, `local-ai-loading-preview-${width}x${height}-${colorScheme}.png`) });

    await page.evaluate(() => window.modelTest.fail('simulated runtime failure'));
    await page.locator('#setupRetryBtn').waitFor({ state: 'visible' });
    assert.match(await page.locator('#setupStatus').textContent(), /Local AI could not start/);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    await page.screenshot({ path: resolve(output, `local-ai-retry-preview-${width}x${height}-${colorScheme}.png`) });
    await page.locator('#setupRetryBtn').click();
    await page.waitForFunction(() => window.modelTest?.starts === 2);
    assert.equal(await page.locator('#setupActions').isVisible(), false);
    assert.deepEqual(errors, []);
    console.log(`${width}x${height} ${colorScheme}: loading, CPU recovery state, error and retry verified.`);
    await page.close();
  }
} finally {
  await browser.close();
}
