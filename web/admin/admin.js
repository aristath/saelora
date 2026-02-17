const $ = (id) => document.getElementById(id)

const TOKEN_KEY = 'saelora_token'
const THEME_KEY = 'saelora_theme'
const THEMES = ['system', 'dark', 'light']

let token = window.localStorage.getItem(TOKEN_KEY) || ''
let currentSettings = null

const loginView = $('loginView')
const adminView = $('adminView')
const loginForm = $('loginForm')
const loginErr = $('loginErr')
const loginEmail = $('loginEmail')
const loginPassword = $('loginPassword')
const adminWho = $('adminWho')
const logoutBtn = $('logoutBtn')

const statsEl = $('stats')
const configJson = $('configJson')
const configNote = $('configNote')
const reloadConfigBtn = $('reloadConfigBtn')
const saveConfigBtn = $('saveConfigBtn')

const refreshInvitesBtn = $('refreshInvitesBtn')
const pendingInvitesEl = $('pendingInvites')
const whitelistInvitesEl = $('whitelistInvites')

const refreshUsersBtn = $('refreshUsersBtn')
const usersTableEl = $('usersTable')

const themeToggle = $('themeToggle')

const ICON_AUTO = 'A'
const ICON_DARK = 'D'
const ICON_LIGHT = 'L'

function getStoredTheme() {
  const t = (window.localStorage.getItem(THEME_KEY) || '').trim()
  if (t === 'light' || t === 'dark') return t
  return 'system'
}

function applyTheme(t) {
  const root = document.documentElement
  if (t === 'light' || t === 'dark') root.dataset.theme = t
  else delete root.dataset.theme

  root.style.colorScheme = t === 'system' ? 'light dark' : t

  if (themeToggle) {
    themeToggle.textContent = t === 'dark' ? ICON_DARK : t === 'light' ? ICON_LIGHT : ICON_AUTO
    themeToggle.title = t === 'system' ? 'Theme: system' : `Theme: ${t}`
  }
}

function cycleTheme() {
  const cur = getStoredTheme()
  const idx = THEMES.indexOf(cur)
  const next = THEMES[(idx + 1) % THEMES.length]
  if (next === 'system') window.localStorage.removeItem(THEME_KEY)
  else window.localStorage.setItem(THEME_KEY, next)
  applyTheme(next)
}

function setToken(t) {
  token = t || ''
  if (token) window.localStorage.setItem(TOKEN_KEY, token)
  else window.localStorage.removeItem(TOKEN_KEY)
}

function setView(name) {
  loginView.classList.toggle('hidden', name !== 'login')
  adminView.classList.toggle('hidden', name !== 'admin')
}

async function apiJson(path, { method = 'GET', body = null, auth = true } = {}) {
  const headers = { 'Content-Type': 'application/json' }
  if (auth && token) headers.Authorization = `Bearer ${token}`
  const res = await fetch(path, {
    method,
    headers,
    body: body ? JSON.stringify(body) : null,
  })

  let data = null
  try {
    data = await res.json()
  } catch {
    // ignore
  }
  return { res, data }
}

function formatErr(data, fallback = 'request failed') {
  return data?.error?.message || fallback
}

function setConfigNote(msg, ok = false) {
  configNote.textContent = msg || ''
  configNote.classList.toggle('ok', !!ok)
}

async function authMe() {
  if (!token) return null
  const { res, data } = await apiJson('/v1/auth/me', { auth: true })
  if (!res.ok) return null
  return data
}

async function login(email, password) {
  const { res, data } = await apiJson('/v1/auth/login', {
    method: 'POST',
    auth: false,
    body: { email, password },
  })
  if (!res.ok) throw new Error(formatErr(data, `login failed (${res.status})`))
  return data?.token || ''
}

async function loadOverview() {
  const { res, data } = await apiJson('/v1/admin/overview')
  if (!res.ok) throw new Error(formatErr(data, `overview failed (${res.status})`))

  adminWho.textContent = data.admin_email || ''
  const items = [
    ['Pending', data.pending_invites ?? 0],
    ['Whitelisted', data.whitelisted ?? 0],
    ['Users', data.users ?? 0],
    ['Messages sent', data.messages_sent ?? 0],
    ['Messages received', data.messages_received ?? 0],
  ]

  statsEl.innerHTML = ''
  for (const [label, value] of items) {
    const card = document.createElement('div')
    card.className = 'stat'
    card.innerHTML = `<div class="stat-label">${label}</div><div class="stat-value">${value}</div>`
    statsEl.appendChild(card)
  }
}

async function loadConfig() {
  const { res, data } = await apiJson('/v1/admin/config')
  if (!res.ok) throw new Error(formatErr(data, `config failed (${res.status})`))

  currentSettings = data.settings || {}
  configJson.value = JSON.stringify(currentSettings, null, 2)
}

async function saveConfig() {
  const raw = configJson.value || '{}'
  let settings
  try {
    settings = JSON.parse(raw)
  } catch {
    setConfigNote('invalid JSON', false)
    return
  }

  const { res, data } = await apiJson('/v1/admin/config', {
    method: 'POST',
    body: { settings },
  })
  if (!res.ok) {
    setConfigNote(formatErr(data, `save failed (${res.status})`), false)
    return
  }

  setConfigNote('saved', true)
  await loadConfig()
}

function renderInviteList(listEl, records, actions) {
  listEl.innerHTML = ''
  if (!records || records.length === 0) {
    const li = document.createElement('li')
    li.className = 'muted'
    li.textContent = 'empty'
    listEl.appendChild(li)
    return
  }

  for (const r of records) {
    const li = document.createElement('li')
    li.className = 'item'

    const email = document.createElement('span')
    email.className = 'item-email'
    email.textContent = r.email

    const row = document.createElement('div')
    row.className = 'row'

    for (const a of actions) {
      const b = document.createElement('button')
      b.type = 'button'
      b.className = 'ghost'
      b.textContent = a.label
      b.addEventListener('click', () => a.fn(r.email))
      row.appendChild(b)
    }

    li.appendChild(email)
    li.appendChild(row)
    listEl.appendChild(li)
  }
}

async function loadInvites() {
  const { res, data } = await apiJson('/v1/admin/invites')
  if (!res.ok) throw new Error(formatErr(data, `invites failed (${res.status})`))

  renderInviteList(pendingInvitesEl, data.pending || [], [
    { label: 'approve', fn: approveInvite },
    { label: 'remove', fn: removePendingInvite },
  ])

  renderInviteList(whitelistInvitesEl, data.whitelist || [], [
    { label: 'remove', fn: removeWhitelistInvite },
  ])
}

async function approveInvite(email) {
  const { res, data } = await apiJson('/v1/admin/invites/approve', {
    method: 'POST',
    body: { email },
  })
  if (!res.ok) {
    setConfigNote(formatErr(data, `approve failed (${res.status})`), false)
    return
  }
  await Promise.all([loadInvites(), loadOverview()])
}

async function removePendingInvite(email) {
  const { res, data } = await apiJson('/v1/admin/invites/remove', {
    method: 'POST',
    body: { email, list: 'pending' },
  })
  if (!res.ok) {
    setConfigNote(formatErr(data, `remove failed (${res.status})`), false)
    return
  }
  await Promise.all([loadInvites(), loadOverview()])
}

async function removeWhitelistInvite(email) {
  const { res, data } = await apiJson('/v1/admin/invites/remove', {
    method: 'POST',
    body: { email, list: 'whitelist' },
  })
  if (!res.ok) {
    setConfigNote(formatErr(data, `remove failed (${res.status})`), false)
    return
  }
  await Promise.all([loadInvites(), loadOverview()])
}

async function loadUsers() {
  const { res, data } = await apiJson('/v1/admin/users')
  if (!res.ok) throw new Error(formatErr(data, `users failed (${res.status})`))

  const users = data.users || []
  if (users.length === 0) {
    usersTableEl.innerHTML = '<p class="muted">No users</p>'
    return
  }

  const table = document.createElement('table')
  table.className = 'users-table'
  table.innerHTML = `
    <thead>
      <tr>
        <th>Email</th>
        <th>Status</th>
        <th>Action</th>
      </tr>
    </thead>
    <tbody></tbody>
  `

  const tbody = table.querySelector('tbody')
  for (const u of users) {
    const tr = document.createElement('tr')

    const tdEmail = document.createElement('td')
    tdEmail.textContent = u.email

    const tdStatus = document.createElement('td')
    const sel = document.createElement('select')
    for (const st of ['active', 'pending', 'disabled']) {
      const opt = document.createElement('option')
      opt.value = st
      opt.textContent = st
      if (st === u.status) opt.selected = true
      sel.appendChild(opt)
    }
    tdStatus.appendChild(sel)

    const tdAction = document.createElement('td')
    const btn = document.createElement('button')
    btn.type = 'button'
    btn.className = 'ghost'
    btn.textContent = 'save'
    btn.addEventListener('click', async () => {
      const status = sel.value
      const { res, data } = await apiJson('/v1/admin/users/status', {
        method: 'POST',
        body: { user_id: u.id, status },
      })
      if (!res.ok) {
        setConfigNote(formatErr(data, `status update failed (${res.status})`), false)
        return
      }
      await loadUsers()
    })
    tdAction.appendChild(btn)

    tr.appendChild(tdEmail)
    tr.appendChild(tdStatus)
    tr.appendChild(tdAction)
    tbody.appendChild(tr)
  }

  usersTableEl.innerHTML = ''
  usersTableEl.appendChild(table)
}

async function loadAdmin() {
  await Promise.all([loadOverview(), loadConfig(), loadInvites(), loadUsers()])
}

async function enterAdmin() {
  try {
    await loadAdmin()
    setView('admin')
    loginErr.textContent = ''
  } catch (e) {
    setToken('')
    setView('login')
    loginErr.textContent = e.message || 'unauthorized'
  }
}

loginForm?.addEventListener('submit', async (e) => {
  e.preventDefault()
  loginErr.textContent = ''
  const email = (loginEmail.value || '').trim()
  const password = loginPassword.value || ''
  if (!email || !password) {
    loginErr.textContent = 'missing email/password'
    return
  }

  try {
    const t = await login(email, password)
    if (!t) throw new Error('missing token')
    setToken(t)
    await enterAdmin()
  } catch (err) {
    loginErr.textContent = err.message || 'login failed'
  }
})

logoutBtn?.addEventListener('click', async () => {
  try {
    await apiJson('/v1/auth/logout', { method: 'POST' })
  } catch {
    // ignore
  }
  setToken('')
  setView('login')
})

reloadConfigBtn?.addEventListener('click', async () => {
  try {
    await loadConfig()
    setConfigNote('reloaded', true)
  } catch (e) {
    setConfigNote(e.message || 'reload failed', false)
  }
})

saveConfigBtn?.addEventListener('click', async () => {
  await saveConfig()
})

refreshInvitesBtn?.addEventListener('click', async () => {
  try {
    await Promise.all([loadInvites(), loadOverview()])
  } catch (e) {
    setConfigNote(e.message || 'refresh failed', false)
  }
})

refreshUsersBtn?.addEventListener('click', async () => {
  try {
    await loadUsers()
  } catch (e) {
    setConfigNote(e.message || 'refresh failed', false)
  }
})

themeToggle?.addEventListener('click', cycleTheme)

applyTheme(getStoredTheme())

;(async () => {
  const me = await authMe()
  if (!me) {
    setView('login')
    return
  }
  await enterAdmin()
})()
