-- Server-owned Notion integration state. OAuth credentials are never exposed
-- through the Data API: every table has RLS enabled and no user-facing policy.

create table if not exists public.notion_oauth_states (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  state_hash text not null,
  expires_at timestamptz not null,
  consumed_at timestamptz,
  created_at timestamptz not null default now()
);

create index if not exists notion_oauth_states_user_expires_idx
  on public.notion_oauth_states (user_id, expires_at desc);

create table if not exists public.notion_connections (
  user_id uuid primary key references auth.users(id) on delete cascade,
  workspace_id text not null,
  workspace_name text,
  workspace_icon text,
  bot_id text,
  token_ciphertext text not null,
  token_iv text not null,
  encryption_version smallint not null default 1 check (encryption_version = 1),
  connected_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists public.notion_destinations (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.notion_connections(user_id) on delete cascade,
  notion_object_id text not null,
  destination_type text not null check (destination_type in ('page', 'data_source')),
  title text not null,
  title_property text,
  report_mode text not null default 'period_page'
    check (report_mode in ('period_page', 'live_page')),
  is_default boolean not null default false,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (user_id, notion_object_id)
);

create unique index if not exists notion_destinations_one_default_idx
  on public.notion_destinations (user_id)
  where is_default;

create table if not exists public.notion_publications (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  destination_id uuid not null references public.notion_destinations(id) on delete cascade,
  report_mode text not null check (report_mode in ('period_page', 'live_page')),
  period_start date not null,
  period_end date not null,
  policy_version text not null,
  idempotency_key text not null,
  notion_page_id text,
  notion_page_url text,
  status text not null default 'pending'
    check (status in ('pending', 'published', 'failed')),
  failure_code text,
  created_at timestamptz not null default now(),
  published_at timestamptz,
  updated_at timestamptz not null default now(),
  unique (user_id, idempotency_key)
);

create index if not exists notion_publications_recent_idx
  on public.notion_publications (user_id, updated_at desc);

alter table public.notion_oauth_states enable row level security;
alter table public.notion_connections enable row level security;
alter table public.notion_destinations enable row level security;
alter table public.notion_publications enable row level security;

revoke all on table public.notion_oauth_states from anon, authenticated;
revoke all on table public.notion_connections from anon, authenticated;
revoke all on table public.notion_destinations from anon, authenticated;
revoke all on table public.notion_publications from anon, authenticated;

grant all on table public.notion_oauth_states to service_role;
grant all on table public.notion_connections to service_role;
grant all on table public.notion_destinations to service_role;
grant all on table public.notion_publications to service_role;

-- Single-use OAuth state consumption is atomic, preventing two callbacks from
-- exchanging the same authorization code concurrently. Only the service role
-- used inside the Edge Function may execute it.
create or replace function public.consume_notion_oauth_state(
  p_state_id uuid,
  p_state_hash text
)
returns table (user_id uuid)
language sql
security definer
set search_path = public
as $$
  update public.notion_oauth_states
     set consumed_at = now()
   where id = p_state_id
     and state_hash = p_state_hash
     and consumed_at is null
     and expires_at > now()
  returning notion_oauth_states.user_id;
$$;

revoke all on function public.consume_notion_oauth_state(uuid, text) from public, anon, authenticated;
grant execute on function public.consume_notion_oauth_state(uuid, text) to service_role;
