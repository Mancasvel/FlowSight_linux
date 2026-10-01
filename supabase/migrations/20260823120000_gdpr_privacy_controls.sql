-- GDPR controls: purpose records, pseudonymous analytics, and bounded cloud retention.

create extension if not exists pgcrypto with schema extensions;

create table if not exists public.privacy_preferences (
  user_id uuid primary key references auth.users(id) on delete cascade,
  notice_version text not null,
  cloud_sync_enabled boolean not null default false,
  cloud_ai_enabled boolean not null default false,
  updated_at timestamptz not null default now()
);

alter table public.privacy_preferences enable row level security;
revoke all on table public.privacy_preferences from anon, authenticated;
grant all on table public.privacy_preferences to service_role;

-- Keep the preference row private while still letting an authenticated RLS
-- policy test the caller's own cloud-sync choice. The caller cannot pass a
-- different user id and learn that person's preference.
create or replace function public.cloud_sync_permitted(p_user_id uuid)
returns boolean
language sql
stable
security definer
set search_path = public, pg_temp
as $$
  select p_user_id = auth.uid()
    and coalesce((
      select p.cloud_sync_enabled
      from public.privacy_preferences p
      where p.user_id = p_user_id
        and p.notice_version = '2026-08-23'
    ), false);
$$;

revoke all on function public.cloud_sync_permitted(uuid) from public, anon;
grant execute on function public.cloud_sync_permitted(uuid) to authenticated, service_role;

-- Remove legacy blanket client grants. In particular, TRUNCATE bypasses RLS
-- and must never be available to anon/authenticated roles. Keep only the DML
-- required by the desktop client and let RLS decide which rows are accessible.
do $$
begin
  if to_regclass('public.profiles') is not null then
    execute 'revoke all on table public.profiles from anon';
    execute 'revoke insert, update, delete, truncate, references, trigger on table public.profiles from authenticated';
    execute 'grant select on table public.profiles to authenticated';
    execute 'grant insert (id, display_name, avatar_url, last_seen_at) on table public.profiles to authenticated';
    execute 'grant update (display_name, avatar_url, last_seen_at) on table public.profiles to authenticated';
  end if;
  if to_regclass('public.licenses') is not null then
    execute 'revoke all on table public.licenses from anon';
    execute 'revoke insert, update, delete, truncate, references, trigger on table public.licenses from authenticated';
    execute 'grant select on table public.licenses to authenticated';
    execute 'drop policy if exists "PM manages license" on public.licenses';
  end if;
  if to_regclass('public.teams') is not null then
    execute 'revoke all on table public.teams from anon';
    execute 'revoke truncate, references, trigger on table public.teams from authenticated';
    execute 'grant select, insert, update, delete on table public.teams to authenticated';
  end if;
  if to_regclass('public.team_members') is not null then
    execute 'revoke all on table public.team_members from anon';
    execute 'revoke truncate, references, trigger on table public.team_members from authenticated';
    execute 'grant select, insert, update, delete on table public.team_members to authenticated';
  end if;
  if to_regclass('public.work_sessions') is not null then
    execute 'revoke all on table public.work_sessions from anon';
    execute 'revoke update, delete, truncate, references, trigger on table public.work_sessions from authenticated';
    execute 'grant select, insert on table public.work_sessions to authenticated';
  end if;
  if to_regclass('public.activity_reports') is not null then
    execute 'revoke all on table public.activity_reports from anon';
    execute 'revoke update, delete, truncate, references, trigger on table public.activity_reports from authenticated';
    execute 'grant select, insert on table public.activity_reports to authenticated';
  end if;
end
$$;

do $$
begin
  if to_regprocedure('public.handle_new_user()') is not null then
    execute 'revoke all on function public.handle_new_user() from public, anon, authenticated';
  end if;
  if to_regprocedure('public.get_license_member_count(uuid)') is not null then
    execute 'revoke all on function public.get_license_member_count(uuid) from public, anon, authenticated';
  end if;
  if to_regprocedure('public.is_license_valid(uuid)') is not null then
    execute 'revoke all on function public.is_license_valid(uuid) from public, anon, authenticated';
  end if;
  if to_regprocedure('public.check_member_limit()') is not null then
    execute 'revoke all on function public.check_member_limit() from public, anon, authenticated';
  end if;
  if to_regprocedure('public.check_license()') is not null then
    execute 'revoke all on function public.check_license() from public, anon, authenticated';
  end if;
end
$$;

-- Existing permissive RLS policies remain in place, but these restrictive
-- policies make the purpose choice mandatory for every new cloud activity row.
-- Dynamic DDL keeps this migration usable in environments where the legacy
-- application tables have not yet been provisioned.
do $$
begin
  if to_regclass('public.work_sessions') is not null then
    execute 'drop policy if exists "privacy_cloud_sync_guard" on public.work_sessions';
    execute $policy$
      create policy "privacy_cloud_sync_guard"
        on public.work_sessions as restrictive for insert to authenticated
        with check (auth.uid() = user_id and public.cloud_sync_permitted(user_id))
    $policy$;
    execute 'drop policy if exists "privacy_cloud_sync_update_guard" on public.work_sessions';
    execute $policy$
      create policy "privacy_cloud_sync_update_guard"
        on public.work_sessions as restrictive for update to authenticated
        using (auth.uid() = user_id and public.cloud_sync_permitted(user_id))
        with check (auth.uid() = user_id and public.cloud_sync_permitted(user_id))
    $policy$;
  end if;

  if to_regclass('public.activity_reports') is not null then
    execute 'drop policy if exists "privacy_activity_sync_guard" on public.activity_reports';
    execute $policy$
      create policy "privacy_activity_sync_guard"
        on public.activity_reports as restrictive for insert to authenticated
        with check (auth.uid() = user_id and public.cloud_sync_permitted(user_id))
    $policy$;
    execute 'drop policy if exists "privacy_activity_sync_update_guard" on public.activity_reports';
    execute $policy$
      create policy "privacy_activity_sync_update_guard"
        on public.activity_reports as restrictive for update to authenticated
        using (auth.uid() = user_id and public.cloud_sync_permitted(user_id))
        with check (auth.uid() = user_id and public.cloud_sync_permitted(user_id))
    $policy$;
  end if;
end
$$;

-- The identifier below is random and not tied to an account, but remains
-- pseudonymous personal data because the installation can be singled out.
create table if not exists public.anonymous_product_analytics (
  anonymous_id uuid primary key,
  secret_hash text,
  daily_usage jsonb not null default '[]'::jsonb,
  weekly_primary_activity text,
  updated_at timestamptz not null default now(),
  expires_at timestamptz not null default (now() + interval '35 days')
);

alter table public.anonymous_product_analytics
  add column if not exists secret_hash text;

create table if not exists public.product_feedback (
  id uuid primary key default gen_random_uuid(),
  anonymous_id uuid,
  message text not null check (char_length(message) between 3 and 2000),
  app_version text not null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null default (now() + interval '12 months')
);

alter table public.anonymous_product_analytics enable row level security;
alter table public.product_feedback enable row level security;
revoke all on table public.anonymous_product_analytics from anon, authenticated;
revoke all on table public.product_feedback from anon, authenticated;
grant all on table public.anonymous_product_analytics to service_role;
grant all on table public.product_feedback to service_role;

drop function if exists public.upsert_anonymous_product_analytics(uuid, boolean, jsonb, text);
create or replace function public.upsert_anonymous_product_analytics(
  p_anonymous_id uuid,
  p_anonymous_secret text,
  p_consented boolean,
  p_daily_usage jsonb default '[]'::jsonb,
  p_weekly_primary_activity text default null
)
returns void
language plpgsql
security definer
set search_path = public, extensions, pg_temp
as $$
declare
  supplied_hash text;
begin
  if p_anonymous_secret is null or char_length(p_anonymous_secret) < 32 then
    raise exception 'Invalid analytics credential';
  end if;
  supplied_hash := encode(digest(p_anonymous_secret, 'sha256'), 'hex');
  if exists (
    select 1 from public.anonymous_product_analytics a
    where a.anonymous_id = p_anonymous_id
      and a.secret_hash is not null
      and a.secret_hash <> supplied_hash
  ) then
    raise exception 'Invalid analytics credential';
  end if;
  if not p_consented then
    delete from public.product_feedback
      where anonymous_id = p_anonymous_id
        and exists (
          select 1 from public.anonymous_product_analytics a
          where a.anonymous_id = p_anonymous_id
            and (a.secret_hash = supplied_hash or a.secret_hash is null)
        );
    delete from public.anonymous_product_analytics
      where anonymous_id = p_anonymous_id
        and (secret_hash = supplied_hash or secret_hash is null);
    return;
  end if;
  if jsonb_typeof(p_daily_usage) <> 'array' or jsonb_array_length(p_daily_usage) > 31 then
    raise exception 'Invalid analytics payload';
  end if;
  insert into public.anonymous_product_analytics (
    anonymous_id, secret_hash, daily_usage, weekly_primary_activity, updated_at, expires_at
  ) values (
    p_anonymous_id,
    supplied_hash,
    p_daily_usage,
    left(p_weekly_primary_activity, 80),
    now(),
    now() + interval '35 days'
  )
  on conflict (anonymous_id) do update set
    secret_hash = supplied_hash,
    daily_usage = excluded.daily_usage,
    weekly_primary_activity = excluded.weekly_primary_activity,
    updated_at = now(),
    expires_at = now() + interval '35 days'
  where public.anonymous_product_analytics.secret_hash = supplied_hash
     or public.anonymous_product_analytics.secret_hash is null;
end;
$$;

drop function if exists public.submit_product_feedback(text, uuid, text);
create or replace function public.submit_product_feedback(
  p_message text,
  p_anonymous_id uuid default null,
  p_anonymous_secret text default null,
  p_app_version text default 'unknown'
)
returns uuid
language plpgsql
security definer
set search_path = public, extensions, pg_temp
as $$
declare
  created_id uuid;
begin
  if char_length(trim(p_message)) < 3 or char_length(p_message) > 2000 then
    raise exception 'Invalid feedback length';
  end if;
  if p_anonymous_id is not null and not exists (
    select 1 from public.anonymous_product_analytics a
    where a.anonymous_id = p_anonymous_id
      and a.secret_hash = encode(digest(p_anonymous_secret, 'sha256'), 'hex')
  ) then
    raise exception 'Invalid analytics credential';
  end if;
  insert into public.product_feedback (anonymous_id, message, app_version)
  values (p_anonymous_id, trim(p_message), left(p_app_version, 40))
  returning id into created_id;
  return created_id;
end;
$$;

revoke all on function public.upsert_anonymous_product_analytics(uuid, text, boolean, jsonb, text) from public;
revoke all on function public.submit_product_feedback(text, uuid, text, text) from public;
grant execute on function public.upsert_anonymous_product_analytics(uuid, text, boolean, jsonb, text) to anon, authenticated;
grant execute on function public.submit_product_feedback(text, uuid, text, text) to anon, authenticated;

-- Existing application tables receive a purpose-aligned 90-day expiry. The
-- cleanup function is intentionally service-only; schedule it daily in the
-- hosted project (documented in docs/GDPR-COMPLIANCE.md).
alter table if exists public.work_sessions
  add column if not exists expires_at timestamptz default (now() + interval '90 days');
alter table if exists public.activity_reports
  add column if not exists expires_at timestamptz default (now() + interval '90 days');
alter table if exists public.cloud_insights
  add column if not exists expires_at timestamptz default (now() + interval '90 days');
alter table if exists public.prompt_usage
  add column if not exists expires_at timestamptz default (now() + interval '62 days');
alter table if exists public.notion_publications
  add column if not exists expires_at timestamptz default (now() + interval '12 months');

create or replace function public.purge_expired_personal_data()
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  analytics_count integer := 0;
  feedback_count integer := 0;
  work_count integer := 0;
  activity_count integer := 0;
  insight_count integer := 0;
  prompt_usage_count integer := 0;
  notion_state_count integer := 0;
  notion_publication_count integer := 0;
begin
  delete from public.anonymous_product_analytics where expires_at < now();
  get diagnostics analytics_count = row_count;
  delete from public.product_feedback where expires_at < now();
  get diagnostics feedback_count = row_count;
  if to_regclass('public.work_sessions') is not null then
    execute 'delete from public.work_sessions where expires_at < now()';
    get diagnostics work_count = row_count;
  end if;
  if to_regclass('public.activity_reports') is not null then
    execute 'delete from public.activity_reports where expires_at < now()';
    get diagnostics activity_count = row_count;
  end if;
  if to_regclass('public.cloud_insights') is not null then
    execute 'delete from public.cloud_insights where expires_at < now()';
    get diagnostics insight_count = row_count;
  end if;
  if to_regclass('public.prompt_usage') is not null then
    execute 'delete from public.prompt_usage where expires_at < now()';
    get diagnostics prompt_usage_count = row_count;
  end if;
  if to_regclass('public.notion_oauth_states') is not null then
    execute 'delete from public.notion_oauth_states where expires_at < now()';
    get diagnostics notion_state_count = row_count;
  end if;
  if to_regclass('public.notion_publications') is not null then
    execute 'delete from public.notion_publications where expires_at < now()';
    get diagnostics notion_publication_count = row_count;
  end if;
  return jsonb_build_object(
    'anonymous_product_analytics', analytics_count,
    'product_feedback', feedback_count,
    'work_sessions', work_count,
    'activity_reports', activity_count,
    'cloud_insights', insight_count,
    'prompt_usage', prompt_usage_count,
    'notion_oauth_states', notion_state_count,
    'notion_publications', notion_publication_count
  );
end;
$$;

revoke all on function public.purge_expired_personal_data() from public, anon, authenticated;
grant execute on function public.purge_expired_personal_data() to service_role;
