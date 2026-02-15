const $ = (id) => document.getElementById(id)

const form = $('inviteForm')
const emailEl = $('inviteEmail')
const btn = $('inviteBtn')
const statusEl = $('inviteStatus')
const themeToggle = $('themeToggle')

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

function setStatus(text, kind) {
  statusEl.textContent = text
  statusEl.classList.remove('ok', 'bad')
  if (kind) statusEl.classList.add(kind)
}

async function requestInvite(email) {
  const res = await fetch('/invite', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email }),
  })
  let payload = null
  try {
    payload = await res.json()
  } catch {
    // ignore
  }
  if (!res.ok) {
    const msg = payload?.error?.message || `request failed (${res.status})`
    throw new Error(msg)
  }
  return payload
}

if (form) {
  form.addEventListener('submit', async (e) => {
    e.preventDefault()
    const email = (emailEl?.value || '').trim()
    if (!email) return

    btn.disabled = true
    setStatus('Sending…')
    try {
      await requestInvite(email)
      setStatus('Got it. We’ll reach out when it’s your turn.', 'ok')
      form.reset()
      emailEl.blur()
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'error'
      setStatus(msg, 'bad')
    } finally {
      btn.disabled = false
    }
  })
}

applyTheme(getStoredTheme())
if (themeToggle) themeToggle.addEventListener('click', cycleTheme)
