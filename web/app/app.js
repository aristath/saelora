const $ = (id) => document.getElementById(id)

const TOKEN_KEY = 'saelora_token'

const statusEl = $('status')
const logoutBtn = $('logout')
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

const messagesEl = $('messages')
const composerForm = $('composer')
const inputEl = $('input')
const sendBtn = $('send')
const stopBtn = $('stop')

let abortController = null
let token = window.localStorage.getItem(TOKEN_KEY) || ''

// Start with an empty thread; the user initiates.
const messages = []

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

function setStatus(text) {
  statusEl.textContent = text
}

function setView(which) {
  const show = (el, on) => el.classList.toggle('hidden', !on)
  show(viewLogin, which === 'login')
  show(viewSetup, which === 'setup')
  show(viewChat, which === 'chat')
  logoutBtn.classList.toggle('hidden', which !== 'chat')
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

function renderChat() {
  messagesEl.innerHTML = ''
  for (const m of messages) {
    const wrap = document.createElement('div')
    wrap.className = `msg ${m.role}`
    wrap.textContent = m.content

    messagesEl.appendChild(wrap)
  }
  messagesEl.scrollTop = messagesEl.scrollHeight
}

function disableWhileStreaming(on) {
  inputEl.disabled = on
  sendBtn.disabled = on
  stopBtn.disabled = !on
}

function filteredMessages() {
  // Our server only accepts user/assistant roles.
  return messages.filter((m) => m.role === 'user' || m.role === 'assistant')
}

async function health() {
  try {
    const r = await fetch('/healthz')
    setStatus(r.ok ? 'api ok' : 'api down')
  } catch {
    setStatus('api down')
  }
}

async function authMe() {
  if (!token) return null
  const { res, j } = await apiJson('/v1/auth/me', { auth: true })
  if (!res.ok) return null
  return j
}

async function streamChat() {
  abortController = new AbortController()
  disableWhileStreaming(true)
  setStatus('streaming')

  const res = await apiFetch('/v1/chat/completions', {
    method: 'POST',
    auth: true,
    signal: abortController.signal,
    body: {
      stream: true,
      messages: filteredMessages(),
    },
  })

  if (res.status === 401) {
    setToken('')
    setView('login')
    setStatus('unauthorized')
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
          const last = messages[messages.length - 1]
          if (last && last.role === 'assistant') last.content += tok
          renderChat()
        }
      }
    }
  }
}

async function send(text) {
  const t = text.trim()
  if (!t) return

  messages.push({ role: 'user', content: t })
  messages.push({ role: 'assistant', content: '' })
  renderChat()

  try {
    await streamChat()
    setStatus('api ok')
  } catch (e) {
    const msg = e instanceof Error ? e.message : 'error'
    setStatus('error')
    const last = messages[messages.length - 1]
    if (last && last.role === 'assistant' && last.content.trim() === '') {
      messages.pop()
    }
    messages.push({ role: 'assistant', content: `Error: ${msg}` })
    renderChat()
  } finally {
    abortController = null
    disableWhileStreaming(false)
    inputEl.focus()
  }
}

stopBtn.addEventListener('click', () => {
  if (abortController) abortController.abort()
})

composerForm.addEventListener('submit', (e) => {
  e.preventDefault()
  const text = inputEl.value
  inputEl.value = ''
  void send(text)
})

inputEl.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    composerForm.requestSubmit()
  }
})

loginForm.addEventListener('submit', async (e) => {
  e.preventDefault()
  clearErrors()

  const email = (loginEmail.value || '').trim()
  const password = loginPassword.value || ''
  if (!email || !password) {
    loginErr.textContent = 'missing email or password'
    return
  }

  setStatus('logging in')
  const { res, j } = await apiJson('/v1/auth/login', {
    method: 'POST',
    body: { email, password },
  })
  if (!res.ok) {
    loginErr.textContent = j?.error?.message || `login failed (${res.status})`
    setStatus('error')
    return
  }

  setToken(j?.token || '')
  loginPassword.value = ''
  setView('chat')
  renderChat()
  setStatus('api ok')
})

async function requestPasswordLink() {
  clearErrors()
  const email = (loginEmail.value || '').trim()
  if (!email) {
    loginErr.textContent = 'missing email'
    return
  }

  setStatus('requesting')
  const { res, j } = await apiJson('/v1/auth/request-password-link', {
    method: 'POST',
    body: { email },
  })
  if (!res.ok) {
    loginErr.textContent = j?.error?.message || `request failed (${res.status})`
    setStatus('error')
    return
  }
  // The server always replies OK to avoid leaking account eligibility.
  // If the email is eligible, it will receive a link.
  loginErr.textContent = "If you're eligible, you'll receive a link shortly."
  setStatus('check email')
}

passwordLinkBtn.addEventListener('click', () => {
  void requestPasswordLink()
})

registerBtn.addEventListener('click', () => {
  void (async () => {
    clearErrors()
    const email = (loginEmail.value || '').trim()
    const password = loginPassword.value || ''
    if (!email || !password) {
      loginErr.textContent = 'missing email or password'
      return
    }
    if (password.trim().length < 8) {
      loginErr.textContent = 'password must be at least 8 characters'
      return
    }
    setStatus('creating')
    const { res, j } = await apiJson('/v1/auth/register', {
      method: 'POST',
      body: { email, password },
    })
    if (!res.ok) {
      loginErr.textContent = j?.error?.message || `register failed (${res.status})`
      setStatus('error')
      return
    }
    if (j?.token) {
      setToken(j.token)
      setView('chat')
      setStatus('api ok')
      renderChat()
    } else {
      setStatus('ok')
    }
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

  setStatus('saving')
  const { res, j } = await apiJson('/v1/auth/setup', {
    method: 'POST',
    body: { token: tok, password: p1 },
  })
  if (!res.ok) {
    setupErr.textContent = j?.error?.message || `setup failed (${res.status})`
    setStatus('error')
    return
  }

  // Clear fragment so it won't linger in browser history and so refresh lands on login.
  if (j?.token) {
    setToken(j.token)
    window.location.hash = ''
    setupPassword.value = ''
    setupConfirm.value = ''
    setView('chat')
    setStatus('api ok')
    renderChat()
    return
  }

  window.location.hash = ''
  setupPassword.value = ''
  setupConfirm.value = ''
  setStatus('ok')
  setView('login')
})

logoutBtn.addEventListener('click', async () => {
  try {
    await apiJson('/v1/auth/logout', { method: 'POST', auth: true })
  } catch {
    // ignore
  }
  setToken('')
  setView('login')
  setStatus('logged out')
})

async function boot() {
  applyTheme(getStoredTheme())
  if (themeToggle) themeToggle.addEventListener('click', cycleTheme)

  renderChat()
  await health()

  clearErrors()
  const st = setupTokenFromHash()
  if (st) {
    setView('setup')
    setStatus('setup')
    return
  }

  if (token) {
    const me = await authMe()
    if (me) {
      setView('chat')
      setStatus('api ok')
      return
    }
    setToken('')
  }

  setView('login')
  setStatus('login')
}

void boot()
