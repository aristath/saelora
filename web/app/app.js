const $ = (id) => document.getElementById(id)

const TOKEN_KEY = 'saelora_token'

const themeToggle = $('themeToggle')

const viewLogin = $('view-login')
const viewSetup = $('view-setup')
const viewChat = $('view-chat')

const loginForm = $('loginForm')
const loginEmail = $('loginEmail')
const loginPassword = $('loginPassword')
const loginErr = $('loginErr')
const passwordLinkBtn = $('passwordLinkBtn')
const registerBtn = $('registerBtn')

const setupForm = $('setupForm')
const setupPassword = $('setupPassword')
const setupConfirm = $('setupConfirm')
const setupErr = $('setupErr')
const chatCountEl = $('chatCount')

const messagesEl = $('messages')
const composerForm = $('composer')
const inputEl = $('input')
const threadTabsEl = $('threadTabs')
const threadModeWrapEl = $('threadModeWrap')
const threadModeBtnEl = $('threadModeBtn')
const threadModeLabelEl = $('threadModeLabel')
const threadModeMenuEl = $('threadModeMenu')
const addThreadBtn = $('addThreadBtn')
const archiveThreadBtn = $('archiveThreadBtn')

let token = window.localStorage.getItem(TOKEN_KEY) || ''
let streamInFlight = false
const pendingJobs = []
const tickingConversations = new Set()
const conversationStates = new Map()
let conversations = []
let activeConversationId = ''

const THREAD_MODES = ['instant', 'hourly', 'daily', 'weekly']
const THREAD_MODE_SHORT = {
  instant: 'INS',
  hourly: 'HR',
  daily: 'DAY',
  weekly: 'WK',
}

// Theme: default to system, with manual override via localStorage.
const THEME_KEY = 'saelora_theme'
const THEMES = ['system', 'dark', 'light']

const ICON_AUTO =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v18"></path><path d="M3 12h18"></path><path d="M4.6 4.6l14.8 14.8"></path><path d="M19.4 4.6L4.6 19.4"></path></svg>'
const ICON_DARK =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.6A8.5 8.5 0 1 1 11.4 3 7 7 0 0 0 21 12.6z"></path></svg>'
const ICON_LIGHT =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2"></path><path d="M12 20v2"></path><path d="M4.9 4.9l1.4 1.4"></path><path d="M17.7 17.7l1.4 1.4"></path><path d="M2 12h2"></path><path d="M20 12h2"></path><path d="M4.9 19.1l1.4-1.4"></path><path d="M17.7 6.3l1.4-1.4"></path></svg>'

function getStoredTheme() {
  const t = (window.localStorage.getItem(THEME_KEY) || '').trim()
  if (t === 'light' || t === 'dark') return t
  return 'system'
}

function applyTheme(t) {
  const root = document.documentElement
  if (t === 'light' || t === 'dark') root.dataset.theme = t
  else delete root.dataset.theme

  // Give UA hints for native widgets.
  root.style.colorScheme = t === 'system' ? 'light dark' : t

  if (themeToggle) {
    const icon = t === 'dark' ? ICON_DARK : t === 'light' ? ICON_LIGHT : ICON_AUTO
    themeToggle.innerHTML = icon
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

function setView(which) {
  const show = (el, on) => el.classList.toggle('hidden', !on)
  show(viewLogin, which === 'login')
  show(viewSetup, which === 'setup')
  show(viewChat, which === 'chat')
}

function clearErrors() {
  loginErr.textContent = ''
  setupErr.textContent = ''
}

function setToken(t) {
  token = t || ''
  if (token) window.localStorage.setItem(TOKEN_KEY, token)
  else window.localStorage.removeItem(TOKEN_KEY)
}

function setupTokenFromHash() {
  const h = window.location.hash || ''
  const pfx = '#setup='
  if (!h.startsWith(pfx)) return ''
  return decodeURIComponent(h.slice(pfx.length)).trim()
}

async function apiFetch(path, { method = 'GET', body = null, auth = false, signal } = {}) {
  const headers = { 'Content-Type': 'application/json' }
  if (auth && token) headers.Authorization = `Bearer ${token}`

  return await fetch(path, {
    method,
    headers,
    body: body ? JSON.stringify(body) : null,
    signal,
  })
}

async function apiJson(path, opts) {
  const res = await apiFetch(path, opts)

  let j = null
  try {
    j = await res.json()
  } catch {
    // ignore
  }
  return { res, j }
}

function normalizeThreadTitle(v) {
  const s = (v || '').trim()
  return s ? s.slice(0, 120) : 'Untitled'
}

function normalizeThreadMode(v) {
  const m = (v || '').trim().toLowerCase()
  return THREAD_MODES.includes(m) ? m : 'instant'
}

function getConversationState(id) {
  if (!conversationStates.has(id)) {
    conversationStates.set(id, { messages: [], loaded: false })
  }
  return conversationStates.get(id)
}

function currentMessages() {
  if (!activeConversationId) return []
  return getConversationState(activeConversationId).messages
}

function closeThreadModeMenu() {
  if (!threadModeMenuEl || !threadModeBtnEl) return
  threadModeMenuEl.classList.add('hidden')
  threadModeBtnEl.setAttribute('aria-expanded', 'false')
}

function openThreadModeMenu() {
  if (!threadModeMenuEl || !threadModeBtnEl || threadModeBtnEl.disabled) return
  threadModeMenuEl.classList.remove('hidden')
  threadModeBtnEl.setAttribute('aria-expanded', 'true')
}

function setThreadModeMenuState(mode) {
  if (!threadModeMenuEl) return
  const active = normalizeThreadMode(mode)
  const items = threadModeMenuEl.querySelectorAll('.thread-mode-item[data-mode]')
  for (const item of items) {
    const m = normalizeThreadMode(item.getAttribute('data-mode') || '')
    const on = m === active
    item.classList.toggle('active', on)
    item.setAttribute('aria-checked', on ? 'true' : 'false')
  }
}

function activeConversation() {
  if (!activeConversationId) return null
  return conversations.find((c) => c.id === activeConversationId) || null
}

function renderThreads() {
  if (!threadTabsEl) return
  threadTabsEl.innerHTML = ''
  if (archiveThreadBtn) archiveThreadBtn.disabled = !activeConversationId
  if (threadModeBtnEl) {
    const c = activeConversation()
    const mode = c ? normalizeThreadMode(c.mode) : 'instant'
    threadModeBtnEl.disabled = !c
    threadModeBtnEl.title = c ? `Mode: ${mode}` : 'Thread mode'
    threadModeBtnEl.setAttribute('aria-label', c ? `Thread mode: ${mode}` : 'Thread mode')
    if (threadModeLabelEl) {
      threadModeLabelEl.textContent = THREAD_MODE_SHORT[mode] || 'INS'
    }
    setThreadModeMenuState(mode)
    if (!c) closeThreadModeMenu()
  }

  for (const c of conversations) {
    const tab = document.createElement('div')
    tab.className = `thread-tab${c.id === activeConversationId ? ' active' : ''}`
    tab.dataset.id = c.id
    tab.title = c.title

    const title = document.createElement('input')
    title.className = 'thread-title'
    title.type = 'text'
    title.value = c.title
    title.setAttribute('aria-label', 'Thread title')
    title.addEventListener('focus', async () => {
      if (activeConversationId !== c.id) await activateConversation(c.id)
      title.select()
    })
    title.addEventListener('click', (e) => e.stopPropagation())
    title.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        title.blur()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        title.value = c.title
        title.blur()
      }
    })
    title.addEventListener('blur', async () => {
      const next = normalizeThreadTitle(title.value)
      if (next === c.title) {
        title.value = c.title
        return
      }
      const ok = await renameConversation(c.id, next)
      title.value = ok ? next : c.title
      renderThreads()
    })

    tab.addEventListener('click', async () => {
      if (activeConversationId === c.id) return
      await activateConversation(c.id)
    })

    tab.appendChild(title)
    threadTabsEl.appendChild(tab)
  }
}

function renderChat() {
  messagesEl.innerHTML = ''
  const msgs = currentMessages()
  let visibleCount = 0

  for (let i = 0; i < msgs.length; i += 1) {
    const m = msgs[i]
    const isThinking = m.role === 'assistant' && m.content.trim() === ''
    const nick = m.role === 'assistant' ? '<saelora>' : '<you>'
    if (m.content.trim() !== '') visibleCount += 1

    const wrap = document.createElement('div')
    wrap.className = `msg ${m.role}`
    wrap.setAttribute('aria-label', m.role === 'assistant' ? 'Saelora message' : 'Your message')

    const name = document.createElement('span')
    name.className = 'msg-nick'
    name.textContent = nick
    wrap.appendChild(name)

    if (isThinking) {
      wrap.classList.add('thinking')
      const loader = document.createElement('div')
      loader.className = 'loader'
      loader.setAttribute('aria-hidden', 'true')
      wrap.appendChild(loader)

      const sr = document.createElement('span')
      sr.className = 'sr-only'
      sr.textContent = 'Saelora is processing'
      wrap.appendChild(sr)
    } else {
      const content = document.createElement('span')
      content.className = 'msg-content'
      content.textContent = m.content
      wrap.appendChild(content)
    }

    messagesEl.appendChild(wrap)
  }
  if (chatCountEl) chatCountEl.textContent = `Messages: ${visibleCount}`
  messagesEl.scrollTop = messagesEl.scrollHeight
}

function autoSizeInput() {
  if (inputEl.value.length === 0) {
    inputEl.style.removeProperty('height')
    inputEl.style.removeProperty('overflow-y')
    return
  }

  inputEl.style.height = 'auto'
  const styles = window.getComputedStyle(inputEl)
  const computedLineHeight = parseFloat(styles.lineHeight)
  const fontSize = parseFloat(styles.fontSize)
  const lineHeight = Number.isFinite(computedLineHeight) ? computedLineHeight : fontSize * 1.35
  const padTop = parseFloat(styles.paddingTop)
  const padBottom = parseFloat(styles.paddingBottom)
  const maxHeight = lineHeight * 2 + padTop + padBottom
  const nextHeight = Math.min(inputEl.scrollHeight, maxHeight)
  inputEl.style.height = `${nextHeight}px`
  inputEl.style.overflowY = inputEl.scrollHeight > maxHeight ? 'auto' : 'hidden'
}

function filteredMessagesUntil(msgs, indexExclusive) {
  const out = []
  for (let i = 0; i < indexExclusive; i += 1) {
    const m = msgs[i]
    if (m.role !== 'user' && m.role !== 'assistant') continue
    if (typeof m.content !== 'string' || m.content.trim() === '') continue
    out.push({ role: m.role, content: m.content })
  }
  return out
}

async function authMe() {
  if (!token) return null
  const { res, j } = await apiJson('/v1/auth/me', { auth: true })
  if (!res.ok) return null
  return j
}

async function loadConversations() {
  if (!token) return false

  const { res, j } = await apiJson('/v1/chat/conversations', { auth: true })
  if (res.status === 401) {
    setToken('')
    setView('login')
    return false
  }
  if (!res.ok || !Array.isArray(j?.conversations)) {
    return false
  }

  conversations = j.conversations.map((c) => ({
    id: String(c?.id || ''),
    title: normalizeThreadTitle(c?.title || ''),
    mode: normalizeThreadMode(c?.mode),
    created_at: Number(c?.created_at || 0),
    updated_at: Number(c?.updated_at || 0),
  }))
  conversations = conversations.filter((c) => c.id)
  if (conversations.length === 0) {
    const created = await createConversation('New thread')
    if (created) {
      conversations = [created]
      activeConversationId = created.id
    }
  }

  if (!activeConversationId || !conversations.some((c) => c.id === activeConversationId)) {
    activeConversationId = conversations[0]?.id || ''
  }

  const keep = new Set(conversations.map((c) => c.id))
  for (const id of Array.from(conversationStates.keys())) {
    if (!keep.has(id)) conversationStates.delete(id)
  }

  renderThreads()
  return true
}

async function createConversation(title = 'New thread', mode = 'instant') {
  const { res, j } = await apiJson('/v1/chat/conversations', {
    method: 'POST',
    auth: true,
    body: { title, mode: normalizeThreadMode(mode) },
  })
  if (!res.ok) return null
  const c = j?.conversation || {}
  if (!c?.id) return null
  return {
    id: String(c.id),
    title: normalizeThreadTitle(c.title || 'New thread'),
    mode: normalizeThreadMode(c.mode),
    created_at: Number(c.created_at || Date.now()),
    updated_at: Number(c.updated_at || Date.now()),
  }
}

async function renameConversation(id, title) {
  const { res } = await apiJson(`/v1/chat/conversations/${encodeURIComponent(id)}/rename`, {
    method: 'POST',
    auth: true,
    body: { title },
  })
  if (!res.ok) return false
  const idx = conversations.findIndex((c) => c.id === id)
  if (idx >= 0) conversations[idx].title = normalizeThreadTitle(title)
  return true
}

async function archiveConversation(id) {
  const { res } = await apiJson(`/v1/chat/conversations/${encodeURIComponent(id)}/archive`, {
    method: 'POST',
    auth: true,
  })
  return res.ok
}

async function setConversationMode(id, mode) {
  const next = normalizeThreadMode(mode)
  const { res, j } = await apiJson(`/v1/chat/conversations/${encodeURIComponent(id)}/mode`, {
    method: 'POST',
    auth: true,
    body: { mode: next },
  })
  if (!res.ok) return false
  const idx = conversations.findIndex((c) => c.id === id)
  if (idx >= 0) conversations[idx].mode = normalizeThreadMode(j?.mode || next)
  return true
}

async function queueThreadMessage(id, content) {
  const { res, j } = await apiJson(`/v1/chat/conversations/${encodeURIComponent(id)}/message`, {
    method: 'POST',
    auth: true,
    body: { content },
  })
  if (res.status === 401) {
    setToken('')
    setView('login')
    return { ok: false, unauthorized: true }
  }
  if (!res.ok) {
    return { ok: false, error: j?.error?.message || `request failed (${res.status})` }
  }
  return {
    ok: true,
    mode: normalizeThreadMode(j?.mode),
    pendingCount: Number(j?.pending_count || 0),
  }
}

async function tickConversation(id, { quiet = false } = {}) {
  if (!id || tickingConversations.has(id)) return
  tickingConversations.add(id)
  try {
    const { res, j } = await apiJson(`/v1/chat/conversations/${encodeURIComponent(id)}/tick`, {
      method: 'POST',
      auth: true,
    })
    if (res.status === 401) {
      setToken('')
      setView('login')
      return
    }
    if (!res.ok) return
    const idx = conversations.findIndex((c) => c.id === id)
    if (idx >= 0 && j?.mode) conversations[idx].mode = normalizeThreadMode(j.mode)

    const pendingCount = Number(j?.pending_count || 0)
    if (j?.replied && typeof j?.message?.content === 'string' && j.message.content.trim()) {
      const state = getConversationState(id)
      state.messages.push({
        role: 'assistant',
        content: j.message.content,
        ts: Number(j?.message?.created_at || Date.now()),
      })
      if (id === activeConversationId) renderChat()
    }
  } finally {
    tickingConversations.delete(id)
  }
}

function pollNonInstantThreads() {
  if (!token) return
  for (const c of conversations) {
    if (normalizeThreadMode(c.mode) === 'instant') continue
    void tickConversation(c.id, { quiet: true })
  }
}

async function loadHistory(conversationId = activeConversationId) {
  if (!token || !conversationId) return

  const { res, j } = await apiJson(
    `/v1/chat/history?conversation_id=${encodeURIComponent(conversationId)}&limit=2000`,
    { auth: true },
  )
  if (res.status === 401) {
    setToken('')
    setView('login')
    return
  }
  if (!res.ok || !Array.isArray(j?.messages)) return

  const state = getConversationState(conversationId)
  state.messages = []
  for (const m of j.messages) {
    const role = m?.role === 'assistant' ? 'assistant' : 'user'
    const content = typeof m?.content === 'string' ? m.content : ''
    if (!content.trim()) continue
    const created = Number.isFinite(m?.created_at) ? m.created_at : Date.now()
    state.messages.push({ role, content, ts: created })
  }
  state.loaded = true
  if (conversationId === activeConversationId) renderChat()
}

async function activateConversation(id) {
  if (!id) return
  activeConversationId = id
  renderThreads()
  const state = getConversationState(id)
  if (!state.loaded) {
    await loadHistory(id)
  }
  renderChat()
  const c = activeConversation()
  if (c && normalizeThreadMode(c.mode) !== 'instant') {
    void tickConversation(id, { quiet: true })
  }
}

async function initChatSession() {
  const ok = await loadConversations()
  if (!ok) return false
  if (!activeConversationId) {
    renderChat()
    return true
  }
  await activateConversation(activeConversationId)
  return true
}

async function streamChat(job) {

  const state = getConversationState(job.conversationId)
  const res = await apiFetch('/v1/chat/completions', {
    method: 'POST',
    auth: true,
    body: {
      stream: true,
      conversation_id: job.conversationId,
      messages: job.payloadMessages,
    },
  })

  if (res.status === 401) {
    setToken('')
    setView('login')
    throw new Error('unauthorized')
  }

  if (!res.ok) {
    let msg = `request failed (${res.status})`
    try {
      const j = await res.json()
      if (j?.error?.message) msg = j.error.message
    } catch {
      // ignore
    }
    throw new Error(msg)
  }
  if (!res.body) throw new Error('missing response body')

  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''

  while (true) {
    const { value, done } = await reader.read()
    if (done) break

    buf += decoder.decode(value, { stream: true })

    while (true) {
      const idx = buf.indexOf('\n\n')
      if (idx === -1) break
      const rawEvent = buf.slice(0, idx)
      buf = buf.slice(idx + 2)

      const lines = rawEvent.split('\n')
      for (const line of lines) {
        const t = line.trim()
        if (!t.startsWith('data:')) continue
        const data = t.slice('data:'.length).trim()
        if (!data) continue
        if (data === '[DONE]') return

        let chunk
        try {
          chunk = JSON.parse(data)
        } catch {
          continue
        }

        if (chunk?.error?.message) throw new Error(chunk.error.message)

        const tok = chunk?.choices?.[0]?.delta?.content ?? ''
        if (tok) {
          const target = state.messages[job.assistantIndex]
          if (target && target.role === 'assistant') target.content += tok
          if (job.conversationId === activeConversationId) renderChat()
        }
      }
    }
  }
}

async function processPendingJobs() {
  if (streamInFlight) return
  streamInFlight = true

  while (pendingJobs.length > 0) {
    const job = pendingJobs.shift()
    if (!job) continue

    try {
      await streamChat(job)
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'error'
      const state = getConversationState(job.conversationId)
      const target = state.messages[job.assistantIndex]
      if (target && target.role === 'assistant' && target.content.trim() === '') {
        target.content = `Error: ${msg}`
      }
      if (job.conversationId === activeConversationId) renderChat()
    }
  }

  streamInFlight = false
  inputEl.focus()
}

async function send(text) {
  const t = text.trim()
  if (!t) return
  if (!activeConversationId) return

  const thread = activeConversation()
  const mode = normalizeThreadMode(thread?.mode)
  const state = getConversationState(activeConversationId)
  state.messages.push({ role: 'user', content: t, ts: Date.now() })
  if (mode === 'instant') {
    state.messages.push({ role: 'assistant', content: '', ts: Date.now() })
    const assistantIndex = state.messages.length - 1
    const payloadMessages = filteredMessagesUntil(state.messages, assistantIndex)
    pendingJobs.push({
      conversationId: activeConversationId,
      assistantIndex,
      payloadMessages,
    })
  } else {
    const queued = await queueThreadMessage(activeConversationId, t)
    if (!queued.ok) {
      if (!queued.unauthorized) {
        state.messages.push({
          role: 'assistant',
          content: `Error: ${queued.error || 'request failed'}`,
          ts: Date.now(),
        })
      }
    } else {
      if (queued.mode && thread) thread.mode = queued.mode
      void tickConversation(activeConversationId, { quiet: true })
    }
  }

  const idx = conversations.findIndex((c) => c.id === activeConversationId)
  if (idx >= 0) {
    const [c] = conversations.splice(idx, 1)
    conversations.unshift(c)
    renderThreads()
  }

  renderChat()
  if (mode === 'instant') void processPendingJobs()
}

composerForm.addEventListener('submit', (e) => {
  e.preventDefault()
  const text = inputEl.value
  inputEl.value = ''
  autoSizeInput()
  void send(text)
})

inputEl.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    composerForm.requestSubmit()
  }
})

inputEl.addEventListener('input', autoSizeInput)

if (addThreadBtn) {
  addThreadBtn.addEventListener('click', () => {
    void (async () => {
      const c = await createConversation('New thread')
      if (!c) return
      conversations.unshift(c)
      activeConversationId = c.id
      getConversationState(c.id).loaded = false
      renderThreads()
      await activateConversation(c.id)
      inputEl.focus()
    })()
  })
}

if (archiveThreadBtn) {
  archiveThreadBtn.addEventListener('click', () => {
    void (async () => {
      const id = activeConversationId
      if (!id) return
      const ok = await archiveConversation(id)
      if (!ok) return
      conversations = conversations.filter((c) => c.id !== id)
      conversationStates.delete(id)
      for (let i = pendingJobs.length - 1; i >= 0; i -= 1) {
        if (pendingJobs[i]?.conversationId === id) pendingJobs.splice(i, 1)
      }
      if (conversations.length === 0) {
        const created = await createConversation('New thread')
        if (created) conversations = [created]
      }
      activeConversationId = conversations[0]?.id || ''
      renderThreads()
      if (activeConversationId) await activateConversation(activeConversationId)
      else renderChat()
      inputEl.focus()
    })()
  })
}

if (threadModeBtnEl && threadModeMenuEl) {
  threadModeBtnEl.addEventListener('click', () => {
    if (threadModeMenuEl.classList.contains('hidden')) openThreadModeMenu()
    else closeThreadModeMenu()
  })
  threadModeBtnEl.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeThreadModeMenu()
      e.preventDefault()
    }
  })
  threadModeMenuEl.addEventListener('click', (e) => {
    const el = e.target instanceof Element ? e.target.closest('.thread-mode-item[data-mode]') : null
    if (!el) return
    void (async () => {
      const id = activeConversationId
      if (!id) return
      const next = normalizeThreadMode(el.getAttribute('data-mode') || '')
      const ok = await setConversationMode(id, next)
      if (!ok) return
      renderThreads()
      closeThreadModeMenu()
      if (next !== 'instant') void tickConversation(id, { quiet: true })
    })()
  })
  document.addEventListener('click', (e) => {
    if (!threadModeWrapEl) return
    if (e.target instanceof Node && threadModeWrapEl.contains(e.target)) return
    closeThreadModeMenu()
  })
}

loginForm.addEventListener('submit', async (e) => {
  e.preventDefault()
  clearErrors()

  const email = (loginEmail.value || '').trim()
  const password = loginPassword.value || ''
  if (!email || !password) {
    loginErr.textContent = 'missing email or password'
    return
  }

  const { res, j } = await apiJson('/v1/auth/login', {
    method: 'POST',
    body: { email, password },
  })
  if (!res.ok) {
    loginErr.textContent = j?.error?.message || `login failed (${res.status})`
    return
  }

  setToken(j?.token || '')
  loginPassword.value = ''
  setView('chat')
  await initChatSession()
})

async function requestPasswordLink() {
  clearErrors()
  const email = (loginEmail.value || '').trim()
  if (!email) {
    loginErr.textContent = 'missing email'
    return
  }

  const { res, j } = await apiJson('/v1/auth/request-password-link', {
    method: 'POST',
    body: { email },
  })
  if (!res.ok) {
    loginErr.textContent = j?.error?.message || `request failed (${res.status})`
    return
  }
  // The server always replies OK to avoid leaking account eligibility.
  // If the email is eligible, it will receive a link.
  loginErr.textContent = "If you're eligible, you'll receive a link shortly."
}

passwordLinkBtn.addEventListener('click', () => {
  void requestPasswordLink()
})

registerBtn.addEventListener('click', () => {
  void (async () => {
    clearErrors()
    await requestPasswordLink()
  })()
})

setupForm.addEventListener('submit', async (e) => {
  e.preventDefault()
  clearErrors()

  const tok = setupTokenFromHash()
  const p1 = setupPassword.value || ''
  const p2 = setupConfirm.value || ''
  if (!tok) {
    setupErr.textContent = 'missing setup token'
    return
  }
  if (p1.trim().length < 8) {
    setupErr.textContent = 'password must be at least 8 characters'
    return
  }
  if (p1 !== p2) {
    setupErr.textContent = 'passwords do not match'
    return
  }

  const { res, j } = await apiJson('/v1/auth/setup', {
    method: 'POST',
    body: { token: tok, password: p1 },
  })
  if (!res.ok) {
    setupErr.textContent = j?.error?.message || `setup failed (${res.status})`
    return
  }

  // Clear fragment so it won't linger in browser history and so refresh lands on login.
  if (j?.token) {
    setToken(j.token)
    window.location.hash = ''
    setupPassword.value = ''
    setupConfirm.value = ''
    setView('chat')
    await initChatSession()
    return
  }

  window.location.hash = ''
  setupPassword.value = ''
  setupConfirm.value = ''
  setView('login')
})

async function boot() {
  applyTheme(getStoredTheme())
  if (themeToggle) themeToggle.addEventListener('click', cycleTheme)
  window.setInterval(pollNonInstantThreads, 15000)
  autoSizeInput()

  renderChat()

  clearErrors()
  const st = setupTokenFromHash()
  if (st) {
    setView('setup')
    return
  }

  if (token) {
    const me = await authMe()
    if (me) {
      setView('chat')
      await initChatSession()
      return
    }
    setToken('')
  }

  setView('login')
}

void boot()
