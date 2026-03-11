(() => {
  const API_BASE = (window.REVERSE_AF_API_BASE || '/api/v1').replace(/\/$/, '');
  const app = document.getElementById('app');

  const state = {
    apiKey: sessionStorage.getItem('rc_api_key') || '',
    route: location.hash || '#/dashboard',
    threadHotkeysBound: false,
  };

  let modelName = '';
  let backendsCache = null;

  // NDA state: when viewing a project/thread/artifact that belongs to an NDA project,
  // the main content area gets a blueish tint as a visual reminder.
  let _ndaProjectCache = {}; // projectId -> bool
  async function checkProjectNda(projectId) {
    if (!projectId) return false;
    if (projectId in _ndaProjectCache) return _ndaProjectCache[projectId];
    try {
      const projects = await apiFetch('/projects');
      for (const p of projects) {
        _ndaProjectCache[p.id] = !!p.nda;
      }
      return _ndaProjectCache[projectId] || false;
    } catch (_) {
      return false;
    }
  }

  function applyNdaTint(isNda) {
    const mainEl = document.querySelector('.main');
    if (mainEl) {
      mainEl.classList.toggle('nda-active', isNda);
    }
  }

  function applyTheme(theme) {
    document.body.classList.remove('theme-lab', 'theme-print', 'theme-dark');
    if (theme === 'lab') document.body.classList.add('theme-lab');
    if (theme === 'print') document.body.classList.add('theme-print');
    if (theme === 'dark') document.body.classList.add('theme-dark');
  }

  function getSessionSettings() {
    return {
      agent: localStorage.getItem('rc_session_agent') || '',
      route: localStorage.getItem('rc_session_route') || '',
      theme: localStorage.getItem('rc_theme') || 'lab',
    };
  }

  function setSessionSettings({ agent, route, theme }) {
    if (agent !== undefined) localStorage.setItem('rc_session_agent', agent || '');
    if (route !== undefined) localStorage.setItem('rc_session_route', route || '');
    if (theme !== undefined) localStorage.setItem('rc_theme', theme || 'lab');
    applyTheme(localStorage.getItem('rc_theme') || 'lab');
  }

  function setApiKey(key) {
    state.apiKey = key || '';
    if (state.apiKey) {
      sessionStorage.setItem('rc_api_key', state.apiKey);
    } else {
      sessionStorage.removeItem('rc_api_key');
    }
    render();
  }

  async function apiFetch(path, opts = {}) {
    const headers = Object.assign({}, opts.headers || {});
    headers['Content-Type'] = headers['Content-Type'] || 'application/json';
    if (state.apiKey) {
      headers['Authorization'] = `Bearer ${state.apiKey}`;
    }
    const res = await fetch(`${API_BASE}${path}`, {
      method: opts.method || 'GET',
      headers,
      body: opts.body,
    });
    if (!res.ok) {
      if (res.status === 401) {
        setApiKey('');
        throw new Error('Session expired. Please log in again.');
      }
      const text = await res.text();
      throw new Error(text || `${res.status} ${res.statusText}`);
    }
    return res.json();
  }

  async function apiFetchText(path) {
    const headers = {};
    if (state.apiKey) {
      headers['Authorization'] = `Bearer ${state.apiKey}`;
    }
    const res = await fetch(`${API_BASE}${path}`, { headers });
    if (!res.ok) {
      if (res.status === 401) {
        setApiKey('');
        throw new Error('Session expired. Please log in again.');
      }
      const text = await res.text();
      throw new Error(text || `${res.status} ${res.statusText}`);
    }
    return res.text();
  }

  async function apiUpload(path, file) {
    const headers = {};
    if (state.apiKey) {
      headers['Authorization'] = `Bearer ${state.apiKey}`;
    }
    const form = new FormData();
    form.append('file', file, file.name || 'upload');
    const res = await fetch(`${API_BASE}${path}`, {
      method: 'POST',
      headers,
      body: form,
    });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || `${res.status} ${res.statusText}`);
    }
    return res.json();
  }

  function apiUploadWithProgress(path, file, onProgress) {
    const xhr = new XMLHttpRequest();
    const url = `${API_BASE}${path}`;
    const form = new FormData();
    form.append('file', file, file.name || 'upload');

    const promise = new Promise((resolve, reject) => {
      xhr.open('POST', url, true);
      if (state.apiKey) {
        xhr.setRequestHeader('Authorization', `Bearer ${state.apiKey}`);
      }
      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable && onProgress) {
          onProgress(Math.round((e.loaded / e.total) * 100));
        }
      };
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          try {
            resolve(JSON.parse(xhr.responseText || '{}'));
          } catch (e) {
            resolve({});
          }
        } else {
          reject(new Error(xhr.responseText || `${xhr.status} ${xhr.statusText}`));
        }
      };
      xhr.onerror = () => reject(new Error('upload failed'));
      xhr.send(form);
    });

    return {
      promise,
      abort: () => xhr.abort(),
    };
  }

  async function fetchRoutes() {
    try {
      const data = await apiFetch('/llm/backends');
      if (data && Array.isArray(data.routes) && data.routes.length > 0) {
        return data.routes;
      }
    } catch (_) {}
    return ['auto', 'local', 'backend:openai', 'backend:anthropic', 'backend:vertex'];
  }

  async function fetchAgentDefaultRoute(agentName) {
    if (!agentName) return '';
    try {
      const agent = await apiFetch(`/agents/${agentName}`);
      return agent.default_route || '';
    } catch (_) {
      return '';
    }
  }

  async function downloadArtifact(id, filename) {
    const headers = {};
    if (state.apiKey) {
      headers['Authorization'] = `Bearer ${state.apiKey}`;
    }
    const res = await fetch(`${API_BASE}/artifacts/${id}/download`, { headers });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || `${res.status} ${res.statusText}`);
    }
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename || id;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }

  function navigate(hash) {
    location.hash = hash;
  }

  async function copyToClipboard(text) {
    try {
      await navigator.clipboard.writeText(text);
    } catch (_) {
      const textarea = document.createElement('textarea');
      textarea.value = text;
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      textarea.remove();
    }
  }

  function renderLogin() {
    app.innerHTML = `
      <div class="login">
        <h2>Reverse-Arbeiterfarm</h2>
        <p class="muted">Enter your API key to access the workstation.</p>
        <input id="apiKey" class="input" type="password" placeholder="af_xxx" />
        <div style="height:12px"></div>
        <button id="loginBtn" class="button">Login</button>
        <p id="loginErr" class="error"></p>
      </div>
    `;
    const btn = document.getElementById('loginBtn');
    btn.onclick = async () => {
      const val = document.getElementById('apiKey').value.trim();
      if (!val) {
        document.getElementById('loginErr').textContent = 'API key required.';
        return;
      }
      btn.disabled = true;
      btn.textContent = 'Verifying...';
      try {
        const headers = { 'Authorization': `Bearer ${val}` };
        const res = await fetch(`${API_BASE}/projects`, { headers });
        if (res.status === 401 || res.status === 403) {
          document.getElementById('loginErr').textContent = 'Invalid or expired API key.';
          return;
        }
        setApiKey(val);
      } catch (_) {
        // Network error — accept optimistically
        setApiKey(val);
      } finally {
        btn.disabled = false;
        btn.textContent = 'Login';
      }
    };
    document.getElementById('apiKey').addEventListener('keydown', (e) => {
      if (e.key === 'Enter') btn.click();
    });
  }

  function renderShell(contentHtml) {
    const session = getSessionSettings();
    const navItems = [
      ['#/dashboard', 'Dashboard'],
      ['#/projects', 'Projects'],
      ['#/artifacts', 'Artifacts'],
      ['#/threads', 'Conversations'],
      ['#/workflows', 'Workflows'],
      ['#/tools', 'Tools'],
      ['#/agents', 'Agents'],
      ['#/plugins', 'Plugins'],
      ['#/knowledge', 'Knowledge'],
      ['#/yara', 'YARA'],
      ['#/web-rules', 'Web Rules'],
      ['#/email-admin', 'Email'],
      ['#/notifications', 'Notifications'],
      ['#/search', 'Search'],
      ['#/audit', 'Audit'],
      ['#/admin', 'Admin'],
    ];

    const navHtml = navItems.map(([href, label]) => {
      const active = state.route.startsWith(href) ? 'active' : '';
      return `<a class="${active}" href="${href}">${label}</a>`;
    }).join('');

    app.innerHTML = `
      <div class="app-shell">
        <aside class="sidebar">
          <h1>Reverse-Arbeiterfarm</h1>
          <nav class="nav">${navHtml}</nav>
          <div id="sidebarCost" class="muted" style="padding:0 12px;font-size:11px;margin-top:auto;border-top:1px solid var(--border);padding-top:8px;"></div>
        </aside>
        <main class="main">
          <div class="topbar">
            <div class="muted">${state.route.replace('#/', '')}</div>
            <div class="session-bar">
              <input id="sessionAgent" class="input" placeholder="Session agent" value="${escapeHtml(session.agent)}" />
              <input id="sessionRoute" class="input" placeholder="Session route" value="${escapeHtml(session.route)}" />
              <select id="themeSelect" class="input">
                <option value="lab" ${session.theme === 'lab' ? 'selected' : ''}>Lab</option>
                <option value="print" ${session.theme === 'print' ? 'selected' : ''}>Print</option>
                <option value="dark" ${session.theme === 'dark' ? 'selected' : ''}>Dark</option>
              </select>
              <button class="button secondary" id="logoutBtn">Logout</button>
            </div>
          </div>
          ${contentHtml}
        </main>
      </div>
    `;
    document.getElementById('logoutBtn').onclick = () => setApiKey('');
    document.getElementById('themeSelect').onchange = (e) => {
      setSessionSettings({ theme: e.target.value });
    };
    const agentInput = document.getElementById('sessionAgent');
    const routeInput = document.getElementById('sessionRoute');
    agentInput.onchange = () => setSessionSettings({ agent: agentInput.value.trim() });
    routeInput.onchange = () => setSessionSettings({ route: routeInput.value.trim() });

    // Reset NDA tint — project-scoped pages re-apply it after renderShell
    applyNdaTint(false);

    // Fire-and-forget sidebar cost fetch (non-blocking)
    fetchSidebarCost();
  }

  // Cache for sidebar cost data (avoid re-fetching on every navigation)
  let _sidebarCostCache = { monthly: null, ts: 0 };

  async function fetchSidebarCost() {
    const el = document.getElementById('sidebarCost');
    if (!el) return;

    const now = Date.now();
    // Use cached data if less than 30s old
    if (_sidebarCostCache.ts && (now - _sidebarCostCache.ts) < 30000) {
      renderSidebarCost(el, _sidebarCostCache.monthly);
      return;
    }

    try {
      const monthly = await apiFetch('/cost/monthly');
      _sidebarCostCache = { monthly, ts: Date.now() };
      renderSidebarCost(el, monthly);
    } catch (e) {
      // Silently ignore — sidebar cost is non-critical
    }
  }

  function renderSidebarCost(el, monthly) {
    if (!monthly) { el.innerHTML = ''; return; }
    const total = monthly.total_cost_usd;
    const display = total != null ? `$${total.toFixed(2)}` : '—';

    // Group costs by provider from route string (format: "provider:model", e.g. "openai:gpt-4o")
    const provNames = { openai: 'OpenAI', anthropic: 'Anthropic', vertex: 'Vertex AI' };
    const byProv = {};
    if (monthly.breakdown) {
      for (const b of monthly.breakdown) {
        const parts = (b.route || '').split(':');
        const key = parts[0].toLowerCase();
        const prov = provNames[key] || key || 'Other';
        byProv[prov] = (byProv[prov] || 0) + (b.cost_usd || 0);
      }
    }
    const lines = Object.entries(byProv)
      .sort((a, b) => b[1] - a[1])
      .map(([p, c]) => `<div style="display:flex;justify-content:space-between;"><span>${p}</span><span>$${c.toFixed(2)}</span></div>`)
      .join('');

    el.innerHTML = `<div style="padding:4px 0;">
      <div style="margin-bottom:2px;"><strong>This month: ${display}</strong></div>
      ${lines}
    </div>`;
  }

  function renderCostCard(costData) {
    if (!costData || !costData.breakdown || costData.breakdown.length === 0) {
      return '';
    }
    const fmtTokens = (n) => {
      if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M';
      if (n >= 1000) return (n / 1000).toFixed(1) + 'K';
      return String(n);
    };
    const fmtCost = (c) => c != null ? `$${c.toFixed(4)}` : '—';

    // Extract provider from route string (format: "provider:model", e.g. "openai:gpt-4o")
    const providerNames = { openai: 'OpenAI', anthropic: 'Anthropic', vertex: 'Vertex AI' };
    const getProvider = (route) => {
      const parts = (route || '').split(':');
      const key = parts[0].toLowerCase();
      return providerNames[key] || key || 'Other';
    };

    // Group breakdown by provider
    const groups = {};
    for (const b of costData.breakdown) {
      const prov = getProvider(b.route);
      if (!groups[prov]) groups[prov] = [];
      groups[prov].push(b);
    }

    // Render per-provider sections with subtotals
    let rows = '';
    const providerOrder = ['Anthropic', 'OpenAI', 'Vertex AI', 'Other'];
    for (const prov of providerOrder) {
      const items = groups[prov];
      if (!items) continue;

      rows += `<tr><td colspan="6" style="font-weight:bold;padding-top:10px;border-bottom:1px solid var(--border);">${escapeHtml(prov)}</td></tr>`;
      for (const b of items) {
        rows += `
          <tr>
            <td style="padding-left:16px;">${escapeHtml(b.model)}</td>
            <td>${b.call_count}</td>
            <td>${fmtTokens(b.prompt_tokens)}</td>
            <td>${fmtTokens(b.completion_tokens)}</td>
            <td>${fmtTokens(b.cached_read_tokens)}</td>
            <td>${fmtCost(b.cost_usd)}</td>
          </tr>`;
      }
      if (items.length > 1) {
        const sub = items.reduce((a, b) => ({
          calls: a.calls + b.call_count,
          input: a.input + b.prompt_tokens,
          output: a.output + b.completion_tokens,
          cached: a.cached + b.cached_read_tokens,
          cost: (a.cost != null && b.cost_usd != null) ? a.cost + b.cost_usd : null,
        }), { calls: 0, input: 0, output: 0, cached: 0, cost: 0 });
        rows += `
          <tr style="font-style:italic;border-top:1px solid var(--border);">
            <td style="padding-left:16px;">Subtotal</td>
            <td>${sub.calls}</td>
            <td>${fmtTokens(sub.input)}</td>
            <td>${fmtTokens(sub.output)}</td>
            <td>${fmtTokens(sub.cached)}</td>
            <td>${fmtCost(sub.cost)}</td>
          </tr>`;
      }
    }

    const totalRow = `
      <tr style="font-weight:bold;border-top:2px solid var(--border);">
        <td>Total</td>
        <td>${costData.breakdown.reduce((s, b) => s + b.call_count, 0)}</td>
        <td>${fmtTokens(costData.total_prompt_tokens)}</td>
        <td>${fmtTokens(costData.total_completion_tokens)}</td>
        <td>${fmtTokens(costData.total_cached_read_tokens)}</td>
        <td>${fmtCost(costData.total_cost_usd)}</td>
      </tr>
    `;

    return `
      <div class="card">
        <h4>LLM Usage &amp; Cost</h4>
        <table class="table">
          <thead><tr><th>Model</th><th>Calls</th><th>Input</th><th>Output</th><th>Cached</th><th>Est. Cost</th></tr></thead>
          <tbody>${rows}${totalRow}</tbody>
        </table>
      </div>
    `;
  }

  async function renderDashboard() {
    let status = 'unknown';
    try {
      await apiFetch('/health');
      status = 'ok';
    } catch (e) {
      status = 'error';
    }
    const recentThreads = readLocalArray('rc_recent_threads');
    const recentArtifacts = readLocalArray('rc_recent_artifacts');
    const threadRows = recentThreads.map(t => `
      <tr>
        <td><a href="#/thread/${t.id}">${escapeHtml(t.title || '(untitled)')}</a></td>
        <td>${escapeHtml(t.agent || '')}</td>
        <td class="muted">${t.id}</td>
        <td>${new Date(t.viewed_at).toLocaleString()}</td>
      </tr>
    `).join('');
    const artifactRows = recentArtifacts.map(a => `
      <tr>
        <td><a href="#/projects/${a.project_id}/artifacts/${a.id}">${escapeHtml(a.filename || '(unnamed)')}</a></td>
        <td class="muted">${a.id}</td>
        <td>${new Date(a.viewed_at).toLocaleString()}</td>
      </tr>
    `).join('');
    renderShell(`
      <div class="card">
        <h3>System Status</h3>
        <p>API: <span class="badge">${status}</span></p>
      </div>
      <div class="card">
        <h3>Quick Links</h3>
        <div class="row">
          <button class="button" id="dashProjects">Projects</button>
          <button class="button secondary" id="dashThreads">Conversations</button>
        </div>
      </div>
      <div class="card">
        <h3>Recent Conversations</h3>
        <table class="table">
          <thead><tr><th>Title</th><th>Agent</th><th>ID</th><th>Viewed</th></tr></thead>
          <tbody>${threadRows || ''}</tbody>
        </table>
      </div>
      <div class="card">
        <h3>Recent Artifacts</h3>
        <table class="table">
          <thead><tr><th>Filename</th><th>ID</th><th>Viewed</th></tr></thead>
          <tbody>${artifactRows || ''}</tbody>
        </table>
      </div>
    `);
    document.getElementById('dashProjects').onclick = () => navigate('#/projects');
    document.getElementById('dashThreads').onclick = () => navigate('#/threads');
  }

  async function renderArtifacts() {
    renderShell(`
      <div class="card">
        <h3>Artifacts</h3>
        <p class="muted">Select a project to view and upload artifacts.</p>
        <button class="button" id="goToProjects">Go to Projects</button>
      </div>
    `);
    document.getElementById('goToProjects').onclick = () => navigate('#/projects');
  }

  async function renderThreadsHome() {
    renderShell(`
      <div class="card">
        <h3>Conversations</h3>
        <p class="muted">Enter a project ID to view or create conversations.</p>
        <div class="row">
          <input id="threadsProjectId" class="input" placeholder="Project UUID" />
          <button id="goThreads" class="button">Go</button>
        </div>
      </div>
    `);
    document.getElementById('goThreads').onclick = () => {
      const id = document.getElementById('threadsProjectId').value.trim();
      if (id) navigate(`#/threads/${id}`);
    };
  }

  async function renderThreads(projectId) {
    let threads = [];
    let workflows = [];
    let agents = [];
    let artifacts = [];
    try {
      [threads, workflows, agents, artifacts] = await Promise.all([
        apiFetch(`/projects/${projectId}/threads`),
        apiFetch('/workflows').catch(() => []),
        apiFetch('/agents').catch(() => []),
        apiFetch(`/projects/${projectId}/artifacts`).catch(() => []),
      ]);
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }
    // Apply NDA tint (fire-and-forget — uses cache if available)
    checkProjectNda(projectId).then(nda => applyNdaTint(nda));

    const wfOptions = workflows.map(w =>
      `<option value="${escapeHtml(w.name)}">${escapeHtml(w.name)}${w.description ? ' — ' + escapeHtml(w.description) : ''}</option>`
    ).join('');

    const agentOptions = [
      '<option value="default">default</option>',
      ...agents.filter(a => a.name !== 'default').map(a =>
        `<option value="${escapeHtml(a.name)}">${escapeHtml(a.name)}</option>`
      ),
    ].join('');

    // Only show uploaded samples (not tool-generated artifacts) in the selector
    const uploadedSamples = artifacts.filter(a => !a.source_tool_run_id);
    const sampleOptions = [
      '<option value="">(all samples)</option>',
      ...uploadedSamples.map(a => {
        const label = a.filename || a.id;
        return `<option value="${escapeHtml(a.id)}">${escapeHtml(label)}</option>`;
      }),
    ].join('');

    renderShell(`
      <div class="card">
        <h3>Conversations for Project ${projectId}</h3>
        <div style="display:flex;gap:10px;align-items:center;margin-bottom:10px;flex-wrap:wrap;">
          <select id="threadMode" class="input" style="flex:0 0 130px;width:auto;">
            <option value="agent">Agent</option>
            <option value="workflow">Workflow</option>
            <option value="thinking">Thinking</option>
          </select>
          <input id="threadTitle" class="input" style="flex:1 1 180px;min-width:120px;" placeholder="Title (optional)" />
          <span id="threadModeAgent" style="display:contents;">
            <select id="threadAgent" class="input" style="flex:0 0 160px;width:auto;">${agentOptions}</select>
          </span>
          <span id="threadModeWorkflow" style="display:none;">
            <select id="threadWorkflow" class="input" style="flex:0 0 160px;width:auto;">${wfOptions}</select>
          </span>
          <span id="threadModeThinking" style="display:none;">
            <select id="threadThinkAgent" class="input" style="flex:0 0 180px;width:auto;">
              <option value="">thinker (default)</option>
              ${agents.map(a => `<option value="${escapeHtml(a.name)}">${escapeHtml(a.name)}</option>`).join('')}
            </select>
          </span>
          <select id="threadSample" class="input" style="flex:0 1 220px;width:auto;">${sampleOptions}</select>
          <input id="threadPrompt" class="input" style="flex:1 1 180px;min-width:120px;display:none;" placeholder="Prompt (optional)" />
          <input id="threadGoal" class="input" style="flex:1 1 180px;min-width:120px;display:none;" placeholder="Analysis goal..." />
          <button id="createThread" class="button" style="flex:0 0 auto;">Create</button>
        </div>
        <div class="row" style="margin-bottom:10px;">
          <input id="threadFilter" class="input" placeholder="Filter by id, title, or agent" />
          <div class="muted" id="threadCount"></div>
        </div>
        <div class="pagination" id="threadPager"></div>
        <table class="table">
          <thead><tr><th>Title</th><th>Agent</th><th>ID</th><th>Created</th><th></th></tr></thead>
          <tbody id="threadTableBody"></tbody>
        </table>
      </div>
    `);

    const tableBody = document.getElementById('threadTableBody');
    const threadCount = document.getElementById('threadCount');
    const filterInput = document.getElementById('threadFilter');
    const pageSize = 10;
    let page = 1;

    const typeBadge = (type) => {
      if (type === 'thinking') return ' <span class="badge" style="background:#9b59b6;color:#fff;font-size:0.75em;">thinking</span>';
      if (type === 'workflow') return ' <span class="badge" style="background:#2980b9;color:#fff;font-size:0.75em;">workflow</span>';
      return '';
    };
    const renderRows = (items) => items.map(t => `
      <tr>
        <td><a href="#/thread/${t.id}">${escapeHtml(t.title || '(untitled)')}</a>${typeBadge(t.thread_type)}</td>
        <td>${escapeHtml(t.agent_name || '')}</td>
        <td class="muted">${t.id}</td>
        <td>${new Date(t.created_at).toLocaleString()}</td>
        <td><button class="button secondary del-thread" data-id="${t.id}" data-title="${escapeHtml(t.title || '(untitled)')}" style="color:#c00;font-size:0.8em;padding:2px 8px;">Delete</button></td>
      </tr>
    `).join('');

    const renderPage = (items) => {
      const totalPages = Math.max(1, Math.ceil(items.length / pageSize));
      if (page > totalPages) page = totalPages;
      const start = (page - 1) * pageSize;
      const slice = items.slice(start, start + pageSize);
      tableBody.innerHTML = renderRows(slice);
      threadCount.textContent = `${items.length} total · page ${page}/${totalPages}`;
      const pager = document.getElementById('threadPager');
      pager.innerHTML = `
        <button class="button secondary" id="threadPrev" ${page <= 1 ? 'disabled' : ''}>Prev</button>
        <button class="button secondary" id="threadNext" ${page >= totalPages ? 'disabled' : ''}>Next</button>
      `;
      document.getElementById('threadPrev').onclick = () => { page -= 1; renderPage(items); };
      document.getElementById('threadNext').onclick = () => { page += 1; renderPage(items); };
    };

    const applyFilter = () => {
      const q = filterInput.value.trim().toLowerCase();
      const filtered = q
        ? threads.filter(t => {
            const hay = `${t.id} ${t.title || ''} ${t.agent_name || ''}`.toLowerCase();
            return hay.includes(q);
          })
        : threads;
      page = 1;
      renderPage(filtered);
    };

    tableBody.addEventListener('click', async (e) => {
      const btn = e.target.closest('.del-thread');
      if (!btn) return;
      const id = btn.dataset.id;
      const title = btn.dataset.title;
      if (!confirm(`Delete conversation "${title}"?\n\nAll messages in this conversation will be permanently deleted.`)) return;
      try {
        await apiFetch(`/threads/${id}`, { method: 'DELETE' });
        threads = threads.filter(t => t.id !== id);
        applyFilter();
      } catch (e) {
        alert(`Delete failed: ${e.message}`);
      }
    });

    filterInput.oninput = applyFilter;
    applyFilter();

    // Mode toggle: show/hide agent vs workflow vs thinking fields
    const modeSelect = document.getElementById('threadMode');
    const modeAgentSpan = document.getElementById('threadModeAgent');
    const modeWorkflowSpan = document.getElementById('threadModeWorkflow');
    const modeThinkingSpan = document.getElementById('threadModeThinking');
    const promptInput = document.getElementById('threadPrompt');
    const goalInput = document.getElementById('threadGoal');
    modeSelect.onchange = () => {
      const mode = modeSelect.value;
      modeAgentSpan.style.display = mode === 'agent' ? 'contents' : 'none';
      modeWorkflowSpan.style.display = mode === 'workflow' ? 'contents' : 'none';
      modeThinkingSpan.style.display = mode === 'thinking' ? 'contents' : 'none';
      promptInput.style.display = mode === 'workflow' ? '' : 'none';
      goalInput.style.display = mode === 'thinking' ? '' : 'none';
    };

    document.getElementById('createThread').onclick = async () => {
      const title = document.getElementById('threadTitle').value.trim();
      const mode = modeSelect.value;
      const selectedSampleId = document.getElementById('threadSample').value;
      const selectedSample = selectedSampleId
        ? uploadedSamples.find(a => a.id === selectedSampleId)
        : null;
      // Build a sample-scoped instruction for the prompt
      const sampleContext = selectedSample
        ? `Analyze sample "${selectedSample.filename}" (artifact_id: ${selectedSample.id}, sha256: ${selectedSample.sha256}). Focus only on this sample. Start by running file.info, then proceed with analysis using your available tools.`
        : '';

      // Pass target_artifact_id when a sample is selected
      const targetArtifactId = selectedSample ? selectedSample.id : undefined;

      if (mode === 'workflow') {
        const wfName = document.getElementById('threadWorkflow').value;
        if (!wfName) { alert('Select a workflow.'); return; }
        const threadTitle = title || (selectedSample ? `${wfName}: ${selectedSample.filename}` : wfName);
        try {
          const thread = await apiFetch(`/projects/${projectId}/threads`, {
            method: 'POST',
            body: JSON.stringify({ agent_name: 'default', title: threadTitle, thread_type: 'workflow', target_artifact_id: targetArtifactId }),
          });
          // Navigate to thread detail — user runs the workflow manually via "Run Workflow" button
          location.hash = `#/thread/${thread.id}`;
        } catch (e) {
          alert(e.message);
        }
      } else if (mode === 'thinking') {
        const thinkAgent = document.getElementById('threadThinkAgent').value.trim() || 'thinker';
        const threadTitle = title || (selectedSample ? `Think: ${selectedSample.filename}` : 'Thinking thread');
        try {
          const thread = await apiFetch(`/projects/${projectId}/threads`, {
            method: 'POST',
            body: JSON.stringify({ agent_name: thinkAgent, title: threadTitle, thread_type: 'thinking', target_artifact_id: targetArtifactId }),
          });
          // Store goal in localStorage so the "Run Analysis" panel can pre-fill it
          const userGoal = goalInput.value.trim();
          const goal = [sampleContext, userGoal].filter(Boolean).join('\n\n');
          if (goal) {
            localStorage.setItem(`rc_thinking_goal_${thread.id}`, goal);
          }
          location.hash = `#/thread/${thread.id}`;
        } catch (e) {
          alert(e.message);
        }
      } else {
        const agentName = document.getElementById('threadAgent').value.trim() || 'default';
        const threadTitle = title || (selectedSample ? selectedSample.filename : null);
        try {
          const thread = await apiFetch(`/projects/${projectId}/threads`, {
            method: 'POST',
            body: JSON.stringify({ agent_name: agentName, title: threadTitle, target_artifact_id: targetArtifactId }),
          });
          location.hash = `#/thread/${thread.id}`;
        } catch (e) {
          alert(e.message);
        }
      }
    };
  }

  function renderToolInline(msg, evidence = []) {
    const content = msg.content || '';
    const evHtml = evidence.length
      ? `<div style="margin-top:4px;">${evidence.map(e => `<span class="badge">${escapeHtml(e.ref_type)}:${escapeHtml(e.ref_id)}</span>`).join(' ')}</div>`
      : '';
    const title = msg.tool_name || 'tool';
    const ts = new Date(msg.created_at).toLocaleTimeString();
    const formatted = formatJsonBlock(content);
    return `
      <details class="tool-inline">
        <summary>${escapeHtml(title)} <span class="chat-ts">${ts}</span></summary>
        <pre>${escapeHtml(formatted)}</pre>
      </details>${evHtml}`;
  }

  function renderUserRow(msg, evidence = []) {
    const content = msg.content || '';
    const evHtml = evidence.length
      ? `<div style="margin-top:6px;">${evidence.map(e => `<span class="badge">${escapeHtml(e.ref_type)}:${escapeHtml(e.ref_id)}</span>`).join(' ')}</div>`
      : '';
    const ts = new Date(msg.created_at).toLocaleTimeString();
    return `
      <div class="chat-bubble chat-user">
        <div class="chat-ts" style="text-align:right;">${ts}</div>
        <div>${escapeHtml(content)}${evHtml}</div>
      </div>
    `;
  }

  /** Group messages into assistant turns: each group = { assistant msgs + tool msgs merged } */
  function renderGroupedMessages(messages, evidenceMap) {
    let html = '';
    let i = 0;
    while (i < messages.length) {
      const m = messages[i];
      if (m.role === 'user') {
        html += renderUserRow(m, evidenceMap[m.id] || []);
        i++;
        continue;
      }
      // Start of an assistant turn: collect all consecutive assistant+tool messages
      if (m.role === 'assistant' || m.role === 'tool') {
        const ts = new Date(m.created_at).toLocaleTimeString();
        let toolsHtml = '';
        let textHtml = '';
        let evHtml = '';
        while (i < messages.length && (messages[i].role === 'assistant' || messages[i].role === 'tool')) {
          const cur = messages[i];
          const curEv = evidenceMap[cur.id] || [];
          if (cur.role === 'tool') {
            toolsHtml += renderToolInline(cur, curEv);
          } else {
            // assistant message
            const content = cur.content || '';
            if (content.trim()) {
              textHtml += renderMarkdownSafe(content);
            }
            if (curEv.length) {
              evHtml += `<div style="margin-top:6px;">${curEv.map(e => `<span class="badge">${escapeHtml(e.ref_type)}:${escapeHtml(e.ref_id)}</span>`).join(' ')}</div>`;
            }
          }
          i++;
        }
        html += `
          <div class="chat-bubble chat-assistant">
            <div class="chat-ts">${ts}</div>
            ${toolsHtml}${textHtml}${evHtml}
          </div>`;
        continue;
      }
      // fallback: unknown role
      html += `<div class="chat-bubble">${escapeHtml(m.content || '')}</div>`;
      i++;
    }
    return html;
  }

  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function formatAllowedTools(tools) {
    if (!Array.isArray(tools)) return '';
    return tools.join('\n');
  }

  function parseAllowedTools(text) {
    return text
      .split(/[\n,]/)
      .map(s => s.trim())
      .filter(Boolean);
  }

  function findBackendSpec(name) {
    if (!backendsCache || !backendsCache.backends) return null;
    return backendsCache.backends.find(b => b.name === name) || null;
  }

  function formatCtx(n) {
    if (!n) return '';
    if (n >= 1000000) return (n / 1000000).toFixed(1).replace(/\.0$/, '') + 'M';
    return (n / 1000).toFixed(0) + 'K';
  }

  function renderModelSpec(name) {
    const b = findBackendSpec(name);
    const parts = [`<strong>${escapeHtml(name)}</strong>`];
    if (b) {
      if (b.context_window) parts.push(`${formatCtx(b.context_window)} ctx`);
      if (b.max_output_tokens) parts.push(`${formatCtx(b.max_output_tokens)} max out`);
      if (b.cost_per_mtok_input != null && b.cost_per_mtok_output != null) {
        parts.push(`$${b.cost_per_mtok_input}/$${b.cost_per_mtok_output} /Mtok`);
      }
      if (b.supports_vision) parts.push('<span class="badge">vision</span>');
      if (b.knowledge_cutoff) parts.push(`cutoff ${escapeHtml(b.knowledge_cutoff)}`);
    }
    return `<div class="muted" style="margin-bottom:8px;">${parts.join(' &middot; ')}</div>`;
  }

  function parseRoles(text) {
    return text
      .split(/[\n,]/)
      .map(s => s.trim())
      .filter(Boolean);
  }

  function readLocalArray(key) {
    try {
      const raw = localStorage.getItem(key);
      if (!raw) return [];
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed : [];
    } catch (_) {
      return [];
    }
  }

  function writeLocalArray(key, items) {
    localStorage.setItem(key, JSON.stringify(items));
  }

  function pushHistory(key, item, max = 10, idKey = 'id') {
    const items = readLocalArray(key);
    const existing = items.findIndex(i => i[idKey] === item[idKey]);
    if (existing !== -1) items.splice(existing, 1);
    items.unshift(item);
    if (items.length > max) items.length = max;
    writeLocalArray(key, items);
  }

  function formatJsonBlock(content) {
    if (!content) return '';
    try {
      const obj = typeof content === 'string' ? JSON.parse(content) : content;
      return JSON.stringify(obj, null, 2);
    } catch (_) {
      return String(content);
    }
  }

  function isMostlyText(buffer) {
    const bytes = new Uint8Array(buffer);
    const len = Math.min(bytes.length, 4096);
    let printable = 0;
    for (let i = 0; i < len; i++) {
      const b = bytes[i];
      if (b === 9 || b === 10 || b === 13 || (b >= 32 && b <= 126)) printable++;
    }
    return len > 0 && (printable / len) > 0.8;
  }

  function decodeTextPreview(buffer) {
    const dec = new TextDecoder('utf-8', { fatal: false });
    const text = dec.decode(buffer);
    return text.slice(0, 20000);
  }

  function diffLines(aText, bText) {
    const a = (aText || '').split('\n');
    const b = (bText || '').split('\n');
    const max = Math.max(a.length, b.length);
    const out = [];
    for (let i = 0; i < max; i++) {
      const left = a[i] || '';
      const right = b[i] || '';
      if (left === right) {
        out.push(`  ${left}`);
      } else {
        if (left) out.push(`- ${left}`);
        if (right) out.push(`+ ${right}`);
      }
    }
    return out.join('\n');
  }

  function renderMarkdownSafe(text) {
    const escaped = escapeHtml(text || '');
    const lines = escaped.split('\n');
    let html = '';
    let inCode = false;
    let inTable = false;
    let inList = false;
    let listType = '';

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];

      // Code blocks
      if (line.startsWith('```')) {
        if (inTable) { html += '</tbody></table>'; inTable = false; }
        if (inList) { html += listType === 'ul' ? '</ul>' : '</ol>'; inList = false; }
        inCode = !inCode;
        html += inCode ? '<pre><code>' : '</code></pre>';
        continue;
      }
      if (inCode) {
        html += `${line}\n`;
        continue;
      }

      // Table rows (lines containing | pipes)
      const trimmed = line.trim();
      if (trimmed.startsWith('|') && trimmed.endsWith('|')) {
        if (inList) { html += listType === 'ul' ? '</ul>' : '</ol>'; inList = false; }
        const cells = trimmed.slice(1, -1).split('|').map(c => c.trim());
        // Skip separator rows (|---|---|)
        if (cells.every(c => /^[-: ]+$/.test(c))) continue;
        if (!inTable) {
          html += '<table class="md-table"><thead><tr>';
          cells.forEach(c => { html += `<th>${inlineFormat(c)}</th>`; });
          html += '</tr></thead><tbody>';
          inTable = true;
        } else {
          html += '<tr>';
          cells.forEach(c => { html += `<td>${inlineFormat(c)}</td>`; });
          html += '</tr>';
        }
        continue;
      }
      if (inTable) { html += '</tbody></table>'; inTable = false; }

      // Unordered list
      if (/^[\-\*] /.test(trimmed)) {
        if (!inList || listType !== 'ul') {
          if (inList) html += '</ol>';
          html += '<ul>';
          inList = true;
          listType = 'ul';
        }
        html += `<li>${inlineFormat(trimmed.slice(2))}</li>`;
        continue;
      }

      // Ordered list
      const olMatch = trimmed.match(/^(\d+)\.\s+(.*)/);
      if (olMatch) {
        if (!inList || listType !== 'ol') {
          if (inList) html += '</ul>';
          html += '<ol>';
          inList = true;
          listType = 'ol';
        }
        html += `<li>${inlineFormat(olMatch[2])}</li>`;
        continue;
      }
      if (inList) { html += listType === 'ul' ? '</ul>' : '</ol>'; inList = false; }

      // Headings
      if (line.startsWith('### ')) { html += `<h5>${inlineFormat(line.slice(4))}</h5>`; continue; }
      if (line.startsWith('## '))  { html += `<h4>${inlineFormat(line.slice(3))}</h4>`; continue; }
      if (line.startsWith('# '))   { html += `<h3>${inlineFormat(line.slice(2))}</h3>`; continue; }

      // Empty line
      if (!trimmed) { html += '<div style="height:0.5em;"></div>'; continue; }

      // Normal paragraph
      html += `<div>${inlineFormat(line)}</div>`;
    }
    if (inTable) html += '</tbody></table>';
    if (inList) html += listType === 'ul' ? '</ul>' : '</ol>';
    if (inCode) html += '</code></pre>';
    return html;
  }

  function inlineFormat(text) {
    return text
      .replace(/&lt;br\s*\/?&gt;/gi, '<br>')
      .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
      .replace(/\*(.+?)\*/g, '<em>$1</em>')
      .replace(/`(.+?)`/g, '<code>$1</code>');
  }

  async function sendMessageStream(threadId, content, agentName, routeOverride, onEvent, systemPromptOverride, signal) {
    const headers = { 'Content-Type': 'application/json', 'Accept': 'text/event-stream' };
    if (state.apiKey) headers['Authorization'] = `Bearer ${state.apiKey}`;
    const payload = { content, agent_name: agentName || null };
    if (routeOverride) payload.route = routeOverride;
    if (systemPromptOverride) payload.system_prompt_override = systemPromptOverride;
    const res = await fetch(`${API_BASE}/threads/${threadId}/messages`, {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
      signal,
    });
    if (!res.ok) {
      if (res.status === 401) {
        setApiKey('');
        throw new Error('Session expired. Please log in again.');
      }
      const text = await res.text();
      throw new Error(text || `${res.status} ${res.statusText}`);
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const parts = buffer.split('\n\n');
      buffer = parts.pop();
      for (const part of parts) {
        const lines = part.split('\n');
        let event = 'message';
        const dataLines = [];
        for (const line of lines) {
          if (line.startsWith('event:')) event = line.slice(6).trim();
          if (line.startsWith('data:')) dataLines.push(line.slice(5).trim());
        }
        const data = dataLines.join('\n');
        onEvent(event, data);
      }
    }
  }

  async function renderThreadDetail(threadId) {
    // Guard: reject non-string / obviously invalid thread IDs
    if (!threadId || typeof threadId !== 'string' || threadId.length < 32) {
      console.error('renderThreadDetail called with invalid threadId:', threadId);
      return;
    }
    let messages = [];
    let evidenceMap = {};
    let routes = [];
    let agentNames = [];
    let defaultRoute = '';
    let defaultAgent = '';
    let threadProjectId = '';
    let threadTitle = '';
    try {
      let backendsData;
      [messages, routes, backendsData, agentNames] = await Promise.all([
        apiFetch(`/threads/${threadId}/messages`),
        fetchRoutes(),
        apiFetch('/llm/backends').catch(() => null),
        apiFetch('/agents').then(list => list.map(a => a.name)).catch(() => []),
      ]);
      if (backendsData && backendsData.backends) {
        backendsCache = backendsData;
        if (backendsData.backends.length > 0) {
          modelName = backendsData.backends[0].name;
        }
      }
      const exportText = await apiFetchText(`/threads/${threadId}/export?format=json`);
      const exportJson = JSON.parse(exportText);
      if (exportJson && Array.isArray(exportJson.evidence)) {
        for (const ev of exportJson.evidence) {
          const mid = ev.message_id;
          evidenceMap[mid] = evidenceMap[mid] || [];
          evidenceMap[mid].push(ev);
        }
      }
      const threadAgent = exportJson && exportJson.thread ? exportJson.thread.agent_name : '';
      threadProjectId = exportJson && exportJson.thread ? exportJson.thread.project_id : '';
      threadTitle = exportJson && exportJson.thread ? (exportJson.thread.title || '') : '';
      var threadType = exportJson && exportJson.thread ? (exportJson.thread.thread_type || 'agent') : 'agent';
      defaultAgent = threadAgent || '';
      const saved = localStorage.getItem(`rc_thread_route_${threadId}`);
      if (saved) {
        defaultRoute = saved;
      } else if (threadAgent) {
        defaultRoute = await fetchAgentDefaultRoute(threadAgent);
      }
      if (exportJson && exportJson.thread) {
        pushHistory('rc_recent_threads', {
          id: threadId,
          title: exportJson.thread.title || '',
          agent: exportJson.thread.agent_name || '',
          viewed_at: new Date().toISOString(),
        });
      }
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const viewKey = `rc_thread_view_${threadId}`;
    let viewMode = localStorage.getItem(viewKey) || 'full';
    const renderMessages = () => {
      if (viewMode === 'timeline') {
        return messages.map(m => {
          const who = m.agent_name ? `[${m.agent_name}]` : m.role;
          const snippet = (m.content || '').slice(0, 180);
          return `
            <div class="timeline-item">
              <div><strong>${escapeHtml(who)}</strong> · <span class="muted">${new Date(m.created_at).toLocaleString()}</span></div>
              <div class="muted">${escapeHtml(snippet)}</div>
            </div>
          `;
        }).join('');
      }
      return renderGroupedMessages(messages, evidenceMap);
    };

    const routeOptions = routes
      .map(r => `<option value="${escapeHtml(r)}">${escapeHtml(r)}</option>`)
      .join('');

    const agentListOptions = agentNames
      .map(n => `<option value="${escapeHtml(n)}">`)
      .join('');

    const savedSettingsRaw = localStorage.getItem(`rc_thread_settings_${threadId}`);
    let savedSettings = {};
    try { savedSettings = savedSettingsRaw ? JSON.parse(savedSettingsRaw) : {}; } catch (_) {}
    const session = getSessionSettings();
    const initialAgent = savedSettings.agent || defaultAgent || session.agent || '';
    const initialRoute = savedSettings.route || defaultRoute || session.route || '';
    const initialPrefix = savedSettings.prefix || '';
    const applyPrefix = savedSettings.apply_prefix !== false;

    renderShell(`
      <div class="card">
        <h3>Conversation ${threadId}</h3>
        ${modelName ? renderModelSpec(modelName) : ''}
        <div class="row" style="margin-bottom:10px;">
          ${threadProjectId ? `<button id="backToProject" class="button secondary">Back to Project</button>` : ''}
          <button id="exportMd" class="button secondary">Export Markdown</button>
          <button id="exportJson" class="button secondary">Export JSON</button>
          <button id="copyThreadId" class="button secondary">Copy Conversation ID</button>
          <button id="viewFull" class="button secondary">Full</button>
          <button id="viewTimeline" class="button secondary">Timeline</button>
          <button id="runWorkflowBtn" class="button secondary">Run Workflow</button>
          <button id="runAnalysisBtn" class="button secondary">Run Analysis</button>
          <button id="deleteThread" class="button secondary" style="color:#c00;">Delete</button>
        </div>
        <div id="workflowPanel" class="panel" style="display:none;margin-bottom:10px;">
          <div class="panel-header">
            <strong>Run Workflow</strong>
            <button id="closeWorkflowPanel" class="button secondary" style="font-size:0.8em;padding:4px 8px;">Close</button>
          </div>
          <div class="row" style="margin-bottom:8px;">
            <select id="wfPanelSelect" class="input"><option value="">Loading...</option></select>
            <input id="wfPanelPrompt" class="input" placeholder="Prompt (optional)" />
            <input id="wfPanelRoute" class="input" list="routeList" placeholder="Route (optional)" style="max-width:200px;" />
            <button id="wfPanelRun" class="button">Run</button>
          </div>
          <div id="wfPanelLog" class="muted" style="max-height:200px;overflow:auto;font-size:0.85em;"></div>
        </div>
        <div id="analysisPanel" class="panel" style="display:none;margin-bottom:10px;">
          <div class="panel-header">
            <strong>Run Analysis</strong>
            <button id="closeAnalysisPanel" class="button secondary" style="font-size:0.8em;padding:4px 8px;">Close</button>
          </div>
          <div style="margin-bottom:8px;">
            <textarea id="analysisPanelGoal" class="input" rows="3" style="width:100%;resize:vertical;" placeholder="Analysis goal"></textarea>
          </div>
          <div class="row" style="margin-bottom:8px;">
            <input id="analysisPanelAgent" class="input" list="agentList" placeholder="Agent (default: thinker)" style="max-width:200px;" />
            <input id="analysisPanelRoute" class="input" list="routeList" placeholder="Route (optional)" style="max-width:200px;" />
            <button id="analysisPanelRun" class="button">Run</button>
          </div>
          <div id="analysisPanelLog" class="muted" style="max-height:200px;overflow:auto;font-size:0.85em;"></div>
        </div>
        <div style="margin-top:12px;">
          <div id="msgList">${renderMessages()}</div>
          <div id="liveMsg" class="chat-bubble chat-assistant" style="display:none;"></div>
          <div id="contextUsage" style="display:none;font-size:0.78em;color:#888;padding:2px 8px;"></div>
        </div>
        <div id="toolLog" style="display:none;"></div>
        <div id="childThreads" style="display:none;margin-top:12px;"></div>
        <div id="streamPreview" style="display:none;"></div>
        <div class="panel" style="margin-top:12px;">
          <div style="margin-bottom:8px;">
            <textarea id="msgInput" class="input" rows="3" style="width:100%;resize:vertical;" placeholder="Type a message..."></textarea>
          </div>
          <div class="row" style="justify-content:flex-end;gap:8px;">
            <button id="stopStream" class="button" style="display:none;background:#c0392b;border-color:#c0392b;">Stop</button>
            <span id="queueCount" class="muted" style="display:none;align-self:center;font-size:0.85em;"></span>
            <button id="sendMsg" class="button">Send</button>
          </div>
          <details style="margin-top:10px;">
            <summary style="cursor:pointer;font-size:0.85em;color:#888;">Chat Settings</summary>
            <div style="margin-top:8px;">
              <datalist id="agentList">${agentListOptions}</datalist>
              <div class="row" style="margin-bottom:10px;">
                <input id="agentName" class="input" list="agentList" placeholder="Agent (optional)" value="${escapeHtml(initialAgent)}" />
                <input id="routeOverride" class="input" list="routeList" placeholder="Route override (optional)" value="${escapeHtml(initialRoute)}" />
                <datalist id="routeList">${routeOptions}</datalist>
              </div>
              <div style="margin-bottom:10px;">
                <textarea id="systemPrefix" class="input" rows="2" placeholder="System prompt prefix (local)">${escapeHtml(initialPrefix)}</textarea>
              </div>
              <div class="row" style="margin-bottom:10px;justify-content:space-between;">
                <label><input id="applyPrefix" type="checkbox" ${applyPrefix ? 'checked' : ''}/> Apply prefix to message</label>
                <div class="row">
                  <button id="showPrompt" class="button secondary">Show System Prompt</button>
                  <button id="saveSettings" class="button secondary">Save Settings</button>
                </div>
              </div>
              <div id="promptPreview" style="display:none;margin-top:10px;">
                <textarea id="promptPreviewText" class="input" rows="12" style="width:100%;resize:vertical;font-family:monospace;font-size:0.82em;"></textarea>
                <div class="row" style="margin-top:4px;justify-content:space-between;">
                  <span class="muted" style="font-size:0.78em;">Edit above to override the system prompt for the next message</span>
                  <button id="clearPromptOverride" class="button secondary" style="font-size:0.8em;padding:4px 8px;">Clear Override</button>
                </div>
              </div>
            </div>
          </details>
        </div>
      </div>
    `);

    // Apply NDA tint if this thread belongs to an NDA project
    if (threadProjectId) {
      checkProjectNda(threadProjectId).then(nda => applyNdaTint(nda));
    }

    const liveMsg = document.getElementById('liveMsg');
    const msgList = document.getElementById('msgList');

    // Delegated click handler for thinking toggle buttons
    msgList.addEventListener('click', (e) => {
      const btn = e.target.closest('.toggle-thinking');
      if (!btn) return;
      e.preventDefault();
      e.stopPropagation();
      const targetId = btn.dataset.target;
      const block = document.getElementById(targetId);
      if (!block) return;
      const hidden = block.style.display === 'none';
      block.style.display = hidden ? 'block' : 'none';
      btn.textContent = hidden ? 'Hide thinking' : 'Show thinking';
    });
    let liveBuffer = '';
    let reasoningBuffer = '';
    let showReasoning = false;
    let mdRenderPending = null;  // throttle timer for live markdown rendering
    let lastUsage = null;  // last usage event for context annotation
    const MD_RENDER_INTERVAL = 80; // ms

    // Message queue state — queued messages while model is streaming
    let pendingQueue = [];   // { content, domId, agentName, routeOverride, systemOverride }
    let isDraining = false;

    function updateQueueIndicator() {
      const el = document.getElementById('queueCount');
      if (!el) return;
      if (pendingQueue.length > 0) {
        el.textContent = `${pendingQueue.length} queued`;
        el.style.display = '';
      } else {
        el.style.display = 'none';
      }
    }

    function renderPendingUserRow(content, domId) {
      const ts = new Date().toLocaleTimeString();
      return `
        <div id="${domId}" class="chat-bubble chat-user-pending">
          <div class="pending-badge">queued</div>
          <div class="chat-ts" style="text-align:right;">${ts}</div>
          <div>${escapeHtml(content)}</div>
        </div>
      `;
    }

    function transitionPendingToNormal(domId) {
      const el = document.getElementById(domId);
      if (!el) return;
      el.className = 'chat-bubble chat-user';
      const badge = el.querySelector('.pending-badge');
      if (badge) badge.remove();
    }

    function markPendingFailed(domId, reason) {
      const el = document.getElementById(domId);
      if (!el) return;
      el.className = 'chat-bubble chat-user-failed';
      const badge = el.querySelector('.pending-badge');
      if (badge) badge.textContent = reason || 'failed';
    }

    async function queueMessageToDb(threadId, content) {
      return await apiFetch(`/threads/${threadId}/messages/queue`, {
        method: 'POST',
        body: JSON.stringify({ content }),
      });
    }

    document.getElementById('viewFull').onclick = () => {
      viewMode = 'full';
      localStorage.setItem(viewKey, viewMode);
      msgList.innerHTML = renderMessages();
    };
    document.getElementById('viewTimeline').onclick = () => {
      viewMode = 'timeline';
      localStorage.setItem(viewKey, viewMode);
      msgList.innerHTML = renderMessages();
    };

    // Load and render child threads (for thinking/workflow threads)
    apiFetch(`/threads/${threadId}/children`).then(children => {
      if (children && children.length > 0) {
        const el = document.getElementById('childThreads');
        el.style.display = '';
        el.innerHTML = `
          <details open>
            <summary style="cursor:pointer;font-weight:bold;margin-bottom:8px;">
              Child Threads (${children.length})
            </summary>
            <table class="table" style="font-size:0.9em;">
              <thead><tr><th>Agent</th><th>Title</th><th>Created</th></tr></thead>
              <tbody>
                ${children.map(c => `
                  <tr>
                    <td>${escapeHtml(c.agent_name || '')}</td>
                    <td><a href="#/thread/${c.id}">${escapeHtml(c.title || '(untitled)')}</a></td>
                    <td class="muted">${new Date(c.created_at).toLocaleString()}</td>
                  </tr>
                `).join('')}
              </tbody>
            </table>
          </details>
        `;
      }
    }).catch(() => {});

    // Workflow panel handlers
    document.getElementById('runWorkflowBtn').onclick = async () => {
      const panel = document.getElementById('workflowPanel');
      const sel = document.getElementById('wfPanelSelect');
      if (panel.style.display === 'none') {
        panel.style.display = '';
        try {
          const wfs = await apiFetch('/workflows');
          sel.innerHTML = wfs.map(w =>
            `<option value="${escapeHtml(w.name)}">${escapeHtml(w.name)}${w.description ? ' — ' + escapeHtml(w.description) : ''}</option>`
          ).join('');
        } catch (e) {
          sel.innerHTML = `<option value="">Failed to load workflows</option>`;
        }
      } else {
        panel.style.display = 'none';
      }
    };
    document.getElementById('closeWorkflowPanel').onclick = () => {
      document.getElementById('workflowPanel').style.display = 'none';
    };
    document.getElementById('wfPanelRun').onclick = async () => {
      const wfName = document.getElementById('wfPanelSelect').value;
      const prompt = document.getElementById('wfPanelPrompt').value.trim();
      const route = document.getElementById('wfPanelRoute').value.trim();
      if (!wfName) { alert('Select a workflow.'); return; }
      const log = document.getElementById('wfPanelLog');
      log.innerHTML = '<div>Starting workflow...</div>';
      const runBtn = document.getElementById('wfPanelRun');
      runBtn.disabled = true;
      runBtn.textContent = 'Running...';
      workflowActive = true;
      showStreamingUI();
      try {
        await sendWorkflowStream(threadId, wfName, prompt, route || null, (event, data) => {
          if (event === 'agent_event') {
            const p = safeJson(data);
            log.insertAdjacentHTML('beforeend', `<div>agent: ${escapeHtml(p.agent_name || '')}</div>`);
          } else if (event === 'group_complete') {
            const p = safeJson(data);
            log.insertAdjacentHTML('beforeend', `<div>group ${p.group} complete</div>`);
          } else if (event === 'workflow_complete') {
            const p = safeJson(data);
            log.insertAdjacentHTML('beforeend', `<div><strong>workflow complete: ${escapeHtml(p.workflow_name || '')}</strong></div>`);
          } else if (event === 'error') {
            log.insertAdjacentHTML('beforeend', `<div class="error">error: ${escapeHtml(data)}</div>`);
          } else {
            log.insertAdjacentHTML('beforeend', `<div>${escapeHtml(event)}: ${escapeHtml(data)}</div>`);
          }
          log.scrollTop = log.scrollHeight;
        });
        // Refresh messages after workflow completes
        try {
          messages = await apiFetch(`/threads/${threadId}/messages`);
          const exportText = await apiFetchText(`/threads/${threadId}/export?format=json`);
          const exportJson = JSON.parse(exportText);
          evidenceMap = {};
          if (exportJson && Array.isArray(exportJson.evidence)) {
            for (const ev of exportJson.evidence) {
              const mid = ev.message_id;
              evidenceMap[mid] = evidenceMap[mid] || [];
              evidenceMap[mid].push(ev);
            }
          }
          msgList.innerHTML = renderMessages();
        } catch (_) {}
      } catch (e) {
        log.insertAdjacentHTML('beforeend', `<div class="error">${escapeHtml(e.message)}</div>`);
      } finally {
        workflowActive = false;
        const stopBtn = document.getElementById('stopStream');
        if (stopBtn && !isBackgroundActive()) stopBtn.style.display = 'none';
      }
      runBtn.disabled = false;
      runBtn.textContent = 'Run';
    };

    // Analysis panel handlers
    document.getElementById('runAnalysisBtn').onclick = () => {
      const panel = document.getElementById('analysisPanel');
      if (panel.style.display === 'none') {
        panel.style.display = '';
        // Pre-fill goal from localStorage (set at thinking thread creation)
        const savedGoal = localStorage.getItem(`rc_thinking_goal_${threadId}`);
        if (savedGoal) {
          document.getElementById('analysisPanelGoal').value = savedGoal;
        }
      } else {
        panel.style.display = 'none';
      }
    };
    document.getElementById('closeAnalysisPanel').onclick = () => {
      document.getElementById('analysisPanel').style.display = 'none';
    };
    document.getElementById('analysisPanelRun').onclick = async () => {
      const goal = document.getElementById('analysisPanelGoal').value.trim();
      if (!goal) { alert('Enter an analysis goal.'); return; }
      const agent = document.getElementById('analysisPanelAgent').value.trim() || undefined;
      const route = document.getElementById('analysisPanelRoute').value.trim() || undefined;
      const log = document.getElementById('analysisPanelLog');
      const runBtn = document.getElementById('analysisPanelRun');
      log.innerHTML = '<div>Starting analysis...</div>';
      runBtn.disabled = true;
      runBtn.textContent = 'Running...';
      try {
        await apiFetch(`/threads/${threadId}/thinking`, {
          method: 'POST',
          body: JSON.stringify({ goal, agent_name: agent, route }),
        });
        thinkingActive = true;
        showStreamingUI();
        log.insertAdjacentHTML('beforeend', '<div>Analysis started in background. Messages will appear below.</div>');
        // Clear stored goal after successful start
        localStorage.removeItem(`rc_thinking_goal_${threadId}`);
      } catch (e) {
        log.insertAdjacentHTML('beforeend', `<div class="error">${escapeHtml(e.message)}</div>`);
      }
      runBtn.disabled = false;
      runBtn.textContent = 'Run';
    };
    // Auto-open analysis panel for thinking threads with a stored goal
    if (threadType === 'thinking') {
      const savedGoal = localStorage.getItem(`rc_thinking_goal_${threadId}`);
      if (savedGoal && messages.length === 0) {
        document.getElementById('runAnalysisBtn').click();
      }
    }

    document.getElementById('saveSettings').onclick = () => {
      const agent = document.getElementById('agentName').value.trim();
      const route = document.getElementById('routeOverride').value.trim();
      const prefix = document.getElementById('systemPrefix').value.trim();
      const apply = document.getElementById('applyPrefix').checked;
      localStorage.setItem(
        `rc_thread_settings_${threadId}`,
        JSON.stringify({ agent, route, prefix, apply_prefix: apply })
      );
      alert('Saved local settings.');
    };

    let promptPreviewData = null; // stores the original fetched prompt

    async function refreshPromptPreview() {
      const textarea = document.getElementById('promptPreviewText');
      try {
        const agent = document.getElementById('agentName').value.trim();
        const qs = agent ? `?agent=${encodeURIComponent(agent)}` : '';
        const data = await apiFetch(`/threads/${threadId}/prompt-preview${qs}`);
        promptPreviewData = data;
        textarea.value = data.system_prompt;
      } catch (e) {
        textarea.value = `Error: ${e.message}`;
        promptPreviewData = null;
      }
    }

    // Auto-populate on page load
    refreshPromptPreview();

    document.getElementById('showPrompt').onclick = async () => {
      const preview = document.getElementById('promptPreview');
      if (preview.style.display !== 'none') {
        preview.style.display = 'none';
        return;
      }
      await refreshPromptPreview();
      preview.style.display = '';
    };

    document.getElementById('clearPromptOverride').onclick = async () => {
      await refreshPromptPreview();
    };

    const backBtn = document.getElementById('backToProject');
    if (backBtn) {
      backBtn.onclick = () => navigate(`#/threads/${threadProjectId}`);
    }

    document.getElementById('deleteThread').onclick = async () => {
      if (!confirm(`Delete conversation "${threadTitle || threadId}"?\n\nAll messages will be permanently deleted. This cannot be undone.`)) return;
      try {
        await apiFetch(`/threads/${threadId}`, { method: 'DELETE' });
        if (threadProjectId) {
          navigate(`#/threads/${threadProjectId}`);
        } else {
          navigate('#/projects');
        }
      } catch (e) {
        alert(`Delete failed: ${e.message}`);
      }
    };

    document.getElementById('exportMd').onclick = async () => {
      try {
        const text = await apiFetchText(`/threads/${threadId}/export?format=markdown`);
        const blob = new Blob([text], { type: 'text/markdown' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `thread-${threadId}.md`;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('exportJson').onclick = async () => {
      try {
        const text = await apiFetchText(`/threads/${threadId}/export?format=json`);
        const blob = new Blob([text], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `thread-${threadId}.json`;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('copyThreadId').onclick = async () => {
      await copyToClipboard(threadId);
      alert('Copied conversation ID.');
    };

    let activeAbort = null; // AbortController for the current stream
    let workflowActive = false; // true while a workflow is executing
    let thinkingActive = false; // true while a thinking session is running

    function isBackgroundActive() {
      return activeAbort || workflowActive || thinkingActive;
    }

    function showStreamingUI() {
      const stopBtn = document.getElementById('stopStream');
      if (stopBtn) stopBtn.style.display = '';
    }

    function hideStreamingUI() {
      const stopBtn = document.getElementById('stopStream');
      if (stopBtn && !isBackgroundActive()) stopBtn.style.display = 'none';
      activeAbort = null;
      updateQueueIndicator();
    }

    // Core streaming function — extracted from onclick handler
    async function startStream(content, agent, route, systemOverride) {
      activeAbort = new AbortController();
      showStreamingUI();

      // Render user message immediately
      msgList.insertAdjacentHTML('beforeend', renderUserRow({
        role: 'user',
        content: content,
        created_at: new Date().toISOString(),
      }));

      // Show typing indicator
      const typingEl = document.createElement('div');
      typingEl.className = 'chat-bubble chat-assistant typing-indicator';
      typingEl.innerHTML = '<span></span><span></span><span></span>';
      msgList.appendChild(typingEl);
      typingEl.scrollIntoView({ behavior: 'smooth' });
      let typingRemoved = false;
      function removeTyping() {
        if (!typingRemoved) { typingEl.remove(); typingRemoved = true; }
      }

      try {
        await sendMessageStream(threadId, content, agent, route || null, (event, data) => {
          if (event === 'reasoning') {
            removeTyping();
            reasoningBuffer += data;
            liveMsg.style.display = 'block';
            const reasoningHtml = showReasoning
              ? `<div class="reasoning-block" style="color:#999;font-size:0.85em;white-space:pre-wrap;border-left:2px solid #555;padding-left:8px;margin-bottom:8px;">${escapeHtml(reasoningBuffer)}</div>`
              : '';
            const toggleLabel = showReasoning ? 'Hide thinking' : 'Show thinking';
            const updateLiveReasoning = (e) => {
              if (e) { e.preventDefault(); e.stopPropagation(); }
              showReasoning = !showReasoning;
              const rHtml = showReasoning
                ? `<div class="reasoning-block" style="color:#999;font-size:0.85em;white-space:pre-wrap;word-break:break-word;border-left:2px solid #ddd;padding-left:8px;margin-bottom:8px;">${escapeHtml(reasoningBuffer)}</div>`
                : '';
              const tLabel = showReasoning ? 'Hide thinking' : 'Show thinking';
              liveMsg.innerHTML = `<div class="muted">${liveBuffer ? 'streaming' : 'thinking'}<span class="animated-dots">...</span></div>`
                + `<button type="button" class="button secondary" style="font-size:0.75em;padding:2px 8px;margin:4px 0;" id="toggleReasoning">${tLabel}</button>`
                + rHtml
                + (liveBuffer ? `<div class="md-stream">${renderMarkdownSafe(liveBuffer)}</div>` : '');
              document.getElementById('toggleReasoning').onclick = updateLiveReasoning;
            };
            liveMsg.innerHTML = `<div class="muted">thinking<span class="animated-dots">...</span></div>`
              + `<button type="button" class="button secondary" style="font-size:0.75em;padding:2px 8px;margin:4px 0;" id="toggleReasoning">${toggleLabel}</button>`
              + reasoningHtml;
            document.getElementById('toggleReasoning').onclick = updateLiveReasoning;
          } else if (event === 'token') {
            removeTyping();
            liveBuffer += data;
            liveMsg.style.display = 'block';
            // Throttled live markdown rendering
            if (!mdRenderPending) {
              mdRenderPending = setTimeout(() => {
                mdRenderPending = null;
                const rHtml = showReasoning && reasoningBuffer
                  ? `<div class="reasoning-block" style="color:#999;font-size:0.85em;white-space:pre-wrap;border-left:2px solid #555;padding-left:8px;margin-bottom:8px;">${escapeHtml(reasoningBuffer)}</div>`
                  : '';
                const tBtn = reasoningBuffer
                  ? `<button type="button" class="button secondary" style="font-size:0.75em;padding:2px 8px;margin:4px 0;" id="toggleReasoning">${showReasoning ? 'Hide thinking' : 'Show thinking'}</button>`
                  : '';
                liveMsg.innerHTML = `<div class="muted">streaming<span class="animated-dots">...</span></div>${tBtn}${rHtml}<div class="md-stream">${renderMarkdownSafe(liveBuffer)}</div>`;
                if (reasoningBuffer && document.getElementById('toggleReasoning')) {
                  document.getElementById('toggleReasoning').onclick = (e) => {
                    e.preventDefault(); e.stopPropagation();
                    showReasoning = !showReasoning;
                    const rHtml2 = showReasoning
                      ? `<div class="reasoning-block" style="color:#999;font-size:0.85em;white-space:pre-wrap;word-break:break-word;border-left:2px solid #ddd;padding-left:8px;margin-bottom:8px;">${escapeHtml(reasoningBuffer)}</div>`
                      : '';
                    const tBtn2 = `<button type="button" class="button secondary" style="font-size:0.75em;padding:2px 8px;margin:4px 0;" id="toggleReasoning">${showReasoning ? 'Hide thinking' : 'Show thinking'}</button>`;
                    liveMsg.innerHTML = `<div class="muted">streaming<span class="animated-dots">...</span></div>${tBtn2}${rHtml2}<div class="md-stream">${renderMarkdownSafe(liveBuffer)}</div>`;
                  };
                }
                msgList.scrollTop = msgList.scrollHeight;
              }, MD_RENDER_INTERVAL);
            }
          } else if (event === 'tool_start') {
            removeTyping();
            const ts = safeJson(data);
            const toolName = ts.tool_name || 'unknown';
            liveMsg.style.display = 'block';
            liveMsg.innerHTML = `<div class="muted">running <strong>${escapeHtml(toolName)}</strong><span class="animated-dots">...</span></div>`;
            msgList.scrollTop = msgList.scrollHeight;
          } else if (event === 'tool_result') {
            const tr = safeJson(data);
            const toolName = tr.tool_name || 'unknown';
            const ok = tr.success ? 'completed' : 'failed';
            const summary = tr.summary || '';
            liveMsg.style.display = 'block';
            liveMsg.innerHTML = `<div class="muted">${escapeHtml(toolName)} ${ok}${summary ? ': ' + escapeHtml(summary.substring(0, 120)) : ''}</div>`;
            msgList.scrollTop = msgList.scrollHeight;
          } else if (event === 'evidence') {
            // silently ignored
          } else if (event === 'usage') {
            const u = safeJson(data);
            lastUsage = u;
            const ctxEl = document.getElementById('contextUsage');
            if (ctxEl && u.context_window > 0) {
              const pct = (u.prompt_tokens / u.context_window * 100).toFixed(1);
              const fmtK = (n) => n >= 1000 ? (n / 1000).toFixed(1) + 'K' : String(n);
              const cached = u.cached_read_tokens > 0 ? ` (${fmtK(u.cached_read_tokens)} cached)` : '';
              ctxEl.textContent = `context: ${pct}% of ${fmtK(u.context_window)}  \u2502  ${fmtK(u.prompt_tokens)} in + ${fmtK(u.completion_tokens)} out${cached}  (${u.route || ''})`;
              ctxEl.style.display = 'block';
            }
          } else if (event === 'context_compacted') {
            const c = safeJson(data);
            const ctxEl = document.getElementById('contextUsage');
            if (ctxEl) {
              const fmtK = (n) => n >= 1000 ? (n / 1000).toFixed(1) + 'K' : String(n);
              ctxEl.textContent = `\u21BB compacted: summarized ${c.messages_compacted} messages (${fmtK(c.estimated_tokens)} tokens \u2192 context: ${fmtK(c.context_window)})`;
              ctxEl.style.display = 'block';
            }
          } else if (event === 'done') {
            removeTyping();
            if (mdRenderPending) { clearTimeout(mdRenderPending); mdRenderPending = null; }
            const payload = safeJson(data);
            liveMsg.style.display = 'none';
            const ctxDoneEl = document.getElementById('contextUsage');
            if (ctxDoneEl) ctxDoneEl.style.display = 'none';
            // Use streamed tokens if done payload has no content
            const finalContent = (payload.content || '').trim() || liveBuffer.trim();
            liveBuffer = '';
            const doneReasoning = reasoningBuffer;
            reasoningBuffer = '';
            const ts = new Date().toLocaleTimeString();
            const contentHtml = finalContent ? renderMarkdownSafe(finalContent) : '';
            let reasoningHtml = '';
            if (doneReasoning) {
              const rid = 'reasoning-' + Date.now();
              reasoningHtml = `<button type="button" class="button secondary toggle-thinking" data-target="${rid}" style="font-size:0.75em;padding:2px 8px;margin:4px 0;">Show thinking</button>`
                + `<div id="${rid}" class="reasoning-block" style="display:none;color:#999;font-size:0.85em;white-space:pre-wrap;word-break:break-word;border-left:2px solid #ddd;padding-left:8px;margin:8px 0;">${escapeHtml(doneReasoning)}</div>`;
            }
            let usageHtml = '';
            if (lastUsage && lastUsage.context_window > 0) {
              const u = lastUsage;
              const pct = (u.prompt_tokens / u.context_window * 100).toFixed(1);
              const fmtK = (n) => n >= 1000 ? (n / 1000).toFixed(1) + 'K' : String(n);
              usageHtml = `<div style="font-size:0.7em;color:#888;margin-bottom:2px;">context: ${pct}% of ${fmtK(u.context_window)} · ${fmtK(u.prompt_tokens)} in + ${fmtK(u.completion_tokens)} out · ${u.route || ''}</div>`;
            }
            lastUsage = null;
            const msgHtml = `
              <div class="chat-bubble chat-assistant">
                <div class="chat-ts">${ts}</div>
                ${usageHtml}${reasoningHtml}${contentHtml}
              </div>`;
            msgList.insertAdjacentHTML('beforeend', msgHtml);
          } else if (event === 'error') {
            removeTyping();
            msgList.insertAdjacentHTML('beforeend', `
              <div class="chat-bubble chat-assistant">
                <div class="error">Error: ${escapeHtml(data)}</div>
              </div>
            `);
          }
        }, systemOverride, activeAbort.signal);
        // Refresh prompt preview after model finishes (artifacts may have changed)
        refreshPromptPreview();
      } catch (e) {
        removeTyping();
        if (e.name === 'AbortError') {
          // User pressed Stop — show partial content if any
          if (liveBuffer.trim()) {
            const partial = renderMarkdownSafe(liveBuffer);
            liveMsg.style.display = 'none';
            msgList.insertAdjacentHTML('beforeend', `
              <div class="chat-bubble chat-assistant">
                <div style="font-size:0.75em;color:#c0392b;margin-bottom:4px;">Stopped</div>
                ${partial}
              </div>
            `);
          } else {
            liveMsg.style.display = 'none';
            msgList.insertAdjacentHTML('beforeend', `
              <div class="chat-bubble chat-assistant">
                <div style="color:#c0392b;">Stopped</div>
              </div>
            `);
          }
          liveBuffer = '';
          reasoningBuffer = '';
        } else {
          msgList.insertAdjacentHTML('beforeend', `
            <div class="chat-bubble chat-assistant">
              <div class="error">Error: ${escapeHtml(e.message)}</div>
            </div>
          `);
        }
      } finally {
        hideStreamingUI();
        // Drain queued messages after stream completes
        if (pendingQueue.length > 0) {
          drainQueue();
        }
      }
    }

    // Drain queued messages: insert N-1 via /queue, send last via startStream
    async function drainQueue() {
      if (isDraining) return;
      isDraining = true;

      try {
        // Snapshot the current queue (user may queue more during drain)
        const batch = pendingQueue.splice(0, pendingQueue.length);
        updateQueueIndicator();

        if (batch.length === 0) return;

        // If only one message, send it directly via startStream
        if (batch.length === 1) {
          const msg = batch[0];
          // Remove pending bubble — startStream will render a normal one
          const pendingEl = document.getElementById(msg.domId);
          if (pendingEl) pendingEl.remove();
          isDraining = false;
          await startStream(msg.content, msg.agentName, msg.routeOverride, msg.systemOverride);
          return;
        }

        // Insert N-1 messages via /queue endpoint
        for (let i = 0; i < batch.length - 1; i++) {
          const msg = batch[i];
          try {
            await queueMessageToDb(threadId, msg.content);
            transitionPendingToNormal(msg.domId);
          } catch (e) {
            markPendingFailed(msg.domId, 'failed');
            // Push remaining messages back to queue
            for (let j = i + 1; j < batch.length; j++) {
              pendingQueue.unshift(batch[j]);
            }
            updateQueueIndicator();
            isDraining = false;
            return;
          }
        }

        // Last message: send via startStream (triggers LLM, sees all history)
        const lastMsg = batch[batch.length - 1];
        const pendingEl = document.getElementById(lastMsg.domId);
        if (pendingEl) pendingEl.remove();
        isDraining = false;
        await startStream(lastMsg.content, lastMsg.agentName, lastMsg.routeOverride, lastMsg.systemOverride);
      } finally {
        isDraining = false;
      }
    }

    function getMessageParams() {
      const content = document.getElementById('msgInput').value.trim();
      const agent = document.getElementById('agentName').value.trim();
      const route = document.getElementById('routeOverride').value.trim();
      const prefix = document.getElementById('systemPrefix').value.trim();
      const applyPrefixFlag = document.getElementById('applyPrefix').checked;

      if (!content) return null;

      if (route) {
        localStorage.setItem(`rc_thread_route_${threadId}`, route);
      } else {
        localStorage.removeItem(`rc_thread_route_${threadId}`);
      }

      const sendContent = applyPrefixFlag && prefix
        ? `[System Prefix]\n${prefix}\n\n${content}`
        : content;

      // Check if user has edited the system prompt (override)
      const promptTextarea = document.getElementById('promptPreviewText');
      const currentPrompt = promptTextarea ? promptTextarea.value.trim() : '';
      const originalPrompt = promptPreviewData ? promptPreviewData.system_prompt.trim() : '';
      const systemOverride = (currentPrompt && currentPrompt !== originalPrompt) ? currentPrompt : null;

      document.getElementById('msgInput').value = '';

      return { content: sendContent, agent, route, systemOverride };
    }

    document.getElementById('sendMsg').onclick = async () => {
      const params = getMessageParams();
      if (!params) return;

      if (activeAbort) {
        // STREAMING: queue the message instead of stopping
        const domId = 'pending-' + Date.now() + '-' + Math.random().toString(36).slice(2, 6);
        msgList.insertAdjacentHTML('beforeend', renderPendingUserRow(params.content, domId));
        const lastEl = document.getElementById(domId);
        if (lastEl) lastEl.scrollIntoView({ behavior: 'smooth' });
        pendingQueue.push({
          content: params.content,
          domId,
          agentName: params.agent,
          routeOverride: params.route,
          systemOverride: params.systemOverride,
        });
        updateQueueIndicator();
        return;
      }

      if (workflowActive || thinkingActive) {
        // WORKFLOW/THINKING: queue message directly to DB — backend picks it up mid-loop
        try {
          await queueMessageToDb(threadId, params.content);
          // Render as a normal user bubble (backend handles injection, no drain needed)
          msgList.insertAdjacentHTML('beforeend', renderUserRow({
            role: 'user',
            content: params.content,
            created_at: new Date().toISOString(),
          }));
          msgList.lastElementChild.scrollIntoView({ behavior: 'smooth' });
        } catch (e) {
          msgList.insertAdjacentHTML('beforeend', `
            <div class="chat-bubble chat-user-failed">
              <div class="pending-badge">failed</div>
              <div>${escapeHtml(params.content)}</div>
            </div>
          `);
        }
        return;
      }

      // IDLE: start normal stream
      await startStream(params.content, params.agent, params.route, params.systemOverride);
    };

    // Stop button handler
    document.getElementById('stopStream').onclick = () => {
      if (activeAbort) {
        activeAbort.abort();
      }
      // Clear background flags
      thinkingActive = false;
      // Mark all pending messages as stopped
      for (const msg of pendingQueue) {
        markPendingFailed(msg.domId, 'stopped');
      }
      pendingQueue = [];
      updateQueueIndicator();
      if (!isBackgroundActive()) {
        const stopBtn = document.getElementById('stopStream');
        if (stopBtn) stopBtn.style.display = 'none';
      }
    };

    const msgInputEl = document.getElementById('msgInput');
    msgInputEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        document.getElementById('sendMsg').click();
      }
      // Shift+Enter inserts a newline (default textarea behavior)
      if (e.key === 'Escape' && activeAbort) {
        e.preventDefault();
        document.getElementById('stopStream').click();
      }
    });
    if (!state.threadHotkeysBound) {
      document.addEventListener('keydown', (e) => {
        if (e.key === '/' && document.activeElement !== msgInputEl) {
          e.preventDefault();
          msgInputEl.focus();
        }
        if (e.key === 'Escape' && activeAbort) {
          document.getElementById('stopStream').click();
        }
      });
      state.threadHotkeysBound = true;
    }
  }

  function safeJson(data) {
    try {
      return JSON.parse(data);
    } catch (_) {
      return {};
    }
  }

  function renderWorkflowSteps(steps) {
    if (!Array.isArray(steps)) return '<div class="muted">No steps</div>';
    const groups = {};
    for (const step of steps) {
      const g = String(step.group ?? '1');
      groups[g] = groups[g] || [];
      groups[g].push(step);
    }
    const groupKeys = Object.keys(groups).sort((a, b) => Number(a) - Number(b));
    const groupHtml = groupKeys.map(g => {
      const items = groups[g].map((s, idx) => `
        <div class="wf-step">
          <div class="wf-step-title">#${idx + 1} ${escapeHtml(s.agent || '')}${s.parallel ? ' <span class="badge badge-parallel">parallel</span>' : ''}</div>
          <div class="muted">${escapeHtml(s.prompt || '')}</div>
        </div>
      `).join('');
      return `
        <div class="wf-group">
          <div class="wf-group-title">Group ${g}</div>
          ${items || '<div class="muted">No steps</div>'}
        </div>
      `;
    }).join('');
    return `<div class="wf-groups">${groupHtml}</div>`;
  }

  async function renderWorkflowsHome() {
    let agentNames = [];
    let workflows = [];
    let routes = [];
    try {
      const agents = await apiFetch('/agents');
      agentNames = agents.map(a => a.name);
      workflows = await apiFetch('/workflows');
      routes = await fetchRoutes();
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const wfSources = [...new Set(workflows.map(w => w.source_plugin || 'user'))].sort();
    const wfFilterOpts = wfSources.map(s => `<option value="${escapeHtml(s)}">${escapeHtml(s)}</option>`).join('');

    function buildWorkflowRows(filter) {
      return workflows
        .filter(w => !filter || (w.source_plugin || 'user') === filter)
        .map(w => `
          <tr>
            <td><a href="#/workflows/${w.name}">${w.name}</a></td>
            <td>${escapeHtml(w.source_plugin || 'user')}</td>
            <td>${w.description || ''}</td>
          </tr>
        `).join('');
    }

    const routeOptions = routes
      .map(r => `<option value="${escapeHtml(r)}">${escapeHtml(r)}</option>`)
      .join('');

    const session = getSessionSettings();
    renderShell(`
      <div class="card">
        <h3>Workflows</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="wfThreadId" class="input" placeholder="Conversation UUID to run" />
          <input id="wfName" class="input" placeholder="Workflow name" />
          <input id="wfPrompt" class="input" placeholder="Optional prompt" />
          <input id="wfRoute" class="input" list="routeList" placeholder="Route override (optional)" value="${escapeHtml(session.route || '')}" />
          <datalist id="routeList">${routeOptions}</datalist>
          <button id="runWf" class="button">Run</button>
        </div>
        <div style="margin-bottom:10px;">
          <select id="wfSourceFilter" class="input" style="width:auto;">
            <option value="">All sources</option>
            ${wfFilterOpts}
          </select>
        </div>
        <table class="table">
          <thead><tr><th>Name</th><th>Source</th><th>Description</th></tr></thead>
          <tbody id="wfTableBody">${buildWorkflowRows('')}</tbody>
        </table>
      </div>
      <div class="card">
        <h4>Workflow Events</h4>
        <div id="wfLog" class="muted"></div>
      </div>
      <div class="card">
        <h4>Recent Workflow Runs</h4>
        <div id="wfHistory" class="muted"></div>
      </div>
      <div class="card">
        <h4>Create / Update Workflow</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="wfEditName" class="input" placeholder="Workflow name" />
          <input id="wfEditDesc" class="input" placeholder="Description (optional)" />
        </div>
        <div id="wfSteps"></div>
        <button id="wfAddStep" class="button secondary">Add Step</button>
        <div class="row" style="margin-top:10px;">
          <button id="wfCreate" class="button">Create</button>
          <button id="wfUpdate" class="button secondary">Update</button>
        </div>
      </div>
      <div class="card">
        <h4>Workflow Visualization</h4>
        <div id="wfViz" class="muted">Select a workflow to view steps.</div>
      </div>
    `);

    document.getElementById('wfSourceFilter').onchange = (e) => {
      document.getElementById('wfTableBody').innerHTML = buildWorkflowRows(e.target.value);
    };

    document.getElementById('runWf').onclick = async () => {
      const threadId = document.getElementById('wfThreadId').value.trim();
      const wfName = document.getElementById('wfName').value.trim();
      const prompt = document.getElementById('wfPrompt').value.trim();
      const route = document.getElementById('wfRoute').value.trim();
      if (!threadId || !wfName) return;
      try {
        await executeWorkflowRun(threadId, wfName, prompt, route);
      } catch (e) {
        alert(e.message);
      }
    };

    const wfNameInput = document.getElementById('wfName');
    const wfRouteInput = document.getElementById('wfRoute');
    wfNameInput.onchange = () => {
      const name = wfNameInput.value.trim();
      if (!name || wfRouteInput.value.trim()) return;
      const saved = localStorage.getItem(`rc_workflow_route_${name}`);
      if (saved) {
        wfRouteInput.value = saved;
      }
      renderWorkflowHistory(`rc_workflow_runs_${name}`);
    };
    renderWorkflowHistory(`rc_workflow_runs_${wfNameInput.value.trim() || ''}`);

    initStepsEditor(document.getElementById('wfSteps'), []);

    document.getElementById('wfAddStep').onclick = () => {
      addStepRow(document.getElementById('wfSteps'));
    };

    document.getElementById('wfCreate').onclick = async () => {
      const name = document.getElementById('wfEditName').value.trim();
      const desc = document.getElementById('wfEditDesc').value.trim();
      const steps = collectSteps(document.getElementById('wfSteps'));
      if (!name || steps.length === 0) return;
      const invalid = steps.find(s => !agentNames.includes(s.agent));
      if (invalid) {
        alert(`Unknown agent: ${invalid.agent}`);
        return;
      }
      try {
        await apiFetch('/workflows', {
          method: 'POST',
          body: JSON.stringify({ name, description: desc || null, steps }),
        });
        navigate('#/workflows');
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('wfUpdate').onclick = async () => {
      const name = document.getElementById('wfEditName').value.trim();
      const desc = document.getElementById('wfEditDesc').value.trim();
      const steps = collectSteps(document.getElementById('wfSteps'));
      if (!name || steps.length === 0) return;
      const invalid = steps.find(s => !agentNames.includes(s.agent));
      if (invalid) {
        alert(`Unknown agent: ${invalid.agent}`);
        return;
      }
      try {
        await apiFetch(`/workflows/${name}`, {
          method: 'PUT',
          body: JSON.stringify({ description: desc || null, steps }),
        });
        navigate(`#/workflows/${name}`);
      } catch (e) {
        alert(e.message);
      }
    };
  }

  async function renderWorkflowDetail(name) {
    let agentNames = [];
    let wf;
    let routes = [];
    let defaultRoute = '';
    try {
      const agents = await apiFetch('/agents');
      agentNames = agents.map(a => a.name);
      wf = await apiFetch(`/workflows/${name}`);
      routes = await fetchRoutes();
      const saved = localStorage.getItem(`rc_workflow_route_${name}`);
      if (saved) {
        defaultRoute = saved;
      } else {
        const steps = Array.isArray(wf.steps) ? wf.steps : [];
        const firstAgent = steps.length ? steps[0].agent : '';
        if (firstAgent) {
          defaultRoute = await fetchAgentDefaultRoute(firstAgent);
        }
      }
      if (!defaultRoute) {
        defaultRoute = getSessionSettings().route || '';
      }
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const routeOptions = routes
      .map(r => `<option value="${escapeHtml(r)}">${escapeHtml(r)}</option>`)
      .join('');

    renderShell(`
      <div class="card">
        <h3>Workflow ${escapeHtml(wf.name)}</h3>
        <p class="muted">${escapeHtml(wf.description || '')}</p>
        <div class="row" style="margin-bottom:10px;">
          <input id="wfThreadId" class="input" placeholder="Conversation UUID to run" />
          <input id="wfName" class="input" placeholder="Workflow name" value="${escapeHtml(wf.name)}" />
          <input id="wfPrompt" class="input" placeholder="Optional prompt" />
          <input id="wfRoute" class="input" list="routeList" placeholder="Route override (optional)" value="${escapeHtml(defaultRoute || '')}" />
          <datalist id="routeList">${routeOptions}</datalist>
          <button id="runWf" class="button">Run</button>
        </div>
      </div>
      <div class="card">
        <h4>Workflow Events</h4>
        <div id="wfLog" class="muted"></div>
      </div>
      <div class="card">
        <h4>Recent Workflow Runs</h4>
        <div id="wfHistory" class="muted"></div>
      </div>
      <div class="card">
        <h4>Create / Update Workflow</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="wfEditName" class="input" placeholder="Workflow name" value="${escapeHtml(wf.name)}" />
          <input id="wfEditDesc" class="input" placeholder="Description (optional)" value="${escapeHtml(wf.description || '')}" />
        </div>
        <div id="wfSteps"></div>
        <button id="wfAddStep" class="button secondary">Add Step</button>
        <div class="row" style="margin-top:10px;">
          <button id="wfCreate" class="button">Create</button>
          <button id="wfUpdate" class="button secondary">Update</button>
          <button id="wfDelete" class="button secondary">Delete</button>
        </div>
      </div>
      <div class="card">
        <h4>Workflow Visualization</h4>
        <div id="wfViz">${renderWorkflowSteps(wf.steps)}</div>
      </div>
    `);

    document.getElementById('runWf').onclick = async () => {
      const threadId = document.getElementById('wfThreadId').value.trim();
      const wfName = document.getElementById('wfName').value.trim();
      const prompt = document.getElementById('wfPrompt').value.trim();
      const route = document.getElementById('wfRoute').value.trim();
      if (!threadId || !wfName) return;
      try {
        await executeWorkflowRun(threadId, wfName, prompt, route);
      } catch (e) {
        alert(e.message);
      }
    };

    const stepsRoot = document.getElementById('wfSteps');
    initStepsEditor(stepsRoot, Array.isArray(wf.steps) ? wf.steps : []);
    renderWorkflowHistory(`rc_workflow_runs_${wf.name}`);

    document.getElementById('wfAddStep').onclick = () => {
      addStepRow(stepsRoot);
    };

    document.getElementById('wfCreate').onclick = async () => {
      const nameVal = document.getElementById('wfEditName').value.trim();
      const desc = document.getElementById('wfEditDesc').value.trim();
      const steps = collectSteps(stepsRoot);
      if (!nameVal || steps.length === 0) return;
      const invalid = steps.find(s => !agentNames.includes(s.agent));
      if (invalid) {
        alert(`Unknown agent: ${invalid.agent}`);
        return;
      }
      try {
        await apiFetch('/workflows', {
          method: 'POST',
          body: JSON.stringify({ name: nameVal, description: desc || null, steps }),
        });
        navigate(`#/workflows/${nameVal}`);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('wfUpdate').onclick = async () => {
      const nameVal = document.getElementById('wfEditName').value.trim();
      const desc = document.getElementById('wfEditDesc').value.trim();
      const steps = collectSteps(stepsRoot);
      if (!nameVal || steps.length === 0) return;
      const invalid = steps.find(s => !agentNames.includes(s.agent));
      if (invalid) {
        alert(`Unknown agent: ${invalid.agent}`);
        return;
      }
      try {
        await apiFetch(`/workflows/${nameVal}`, {
          method: 'PUT',
          body: JSON.stringify({ description: desc || null, steps }),
        });
        navigate(`#/workflows/${nameVal}`);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('wfDelete').onclick = async () => {
      if (wf.is_builtin) {
        alert('Cannot delete builtin workflow.');
        return;
      }
      if (!confirm(`Delete workflow '${wf.name}'?`)) return;
      try {
        await apiFetch(`/workflows/${wf.name}`, { method: 'DELETE' });
        navigate('#/workflows');
      } catch (e) {
        alert(e.message);
      }
    };
  }

  function initStepsEditor(root, steps) {
    root.innerHTML = '';
    if (!steps || steps.length === 0) {
      addStepRow(root);
      return;
    }
    for (const step of steps) {
      addStepRow(root, step);
    }
  }

  function addStepRow(root, step = {}) {
    const row = document.createElement('div');
    row.className = 'step-row';
    row.innerHTML = `
      <input class="input step-agent" placeholder="agent" value="${escapeHtml(step.agent || '')}" />
      <input class="input step-group" placeholder="group" value="${escapeHtml(step.group || '')}" />
      <input class="input step-prompt" placeholder="prompt" value="${escapeHtml(step.prompt || '')}" />
      <label class="step-parallel-label" title="Run concurrently with other parallel steps in the same group"><input type="checkbox" class="step-parallel" ${step.parallel ? 'checked' : ''} /> parallel</label>
      <button class="button secondary step-up">Up</button>
      <button class="button secondary step-down">Down</button>
      <button class="button secondary step-remove">Remove</button>
    `;
    row.querySelector('.step-up').onclick = () => {
      const prev = row.previousElementSibling;
      if (prev) {
        root.insertBefore(row, prev);
      }
    };
    row.querySelector('.step-down').onclick = () => {
      const next = row.nextElementSibling;
      if (next) {
        root.insertBefore(next, row);
      }
    };
    row.querySelector('.step-remove').onclick = () => {
      row.remove();
    };
    root.appendChild(row);
  }

  function collectSteps(root) {
    const steps = [];
    root.querySelectorAll('.step-row').forEach(row => {
      const agent = row.querySelector('.step-agent').value.trim();
      const groupRaw = row.querySelector('.step-group').value.trim();
      const prompt = row.querySelector('.step-prompt').value.trim();
      const parallel = row.querySelector('.step-parallel')?.checked || false;
      if (!agent || !groupRaw) return;
      const group = Number(groupRaw);
      const step = {
        agent,
        group: Number.isFinite(group) ? group : groupRaw,
        prompt,
      };
      if (parallel) step.parallel = true;
      steps.push(step);
    });
    return steps;
  }

  function renderWorkflowHistory(key) {
    const host = document.getElementById('wfHistory');
    if (!host) return;
    if (!key) {
      host.textContent = 'No recent runs.';
      return;
    }
    const runs = readLocalArray(key);
    if (!runs.length) {
      host.textContent = 'No recent runs.';
      return;
    }
    host.innerHTML = runs.map(r => `
      <div class="row" style="margin-bottom:6px;">
        <div>${escapeHtml(r.workflow || '')}</div>
        <div class="muted">${new Date(r.ran_at).toLocaleString()}</div>
        <div class="muted">${escapeHtml(r.route || 'auto')}</div>
        <button class="button secondary wf-rerun" data-thread="${r.thread_id || ''}" data-route="${escapeHtml(r.route || '')}">Use</button>
      </div>
    `).join('');
    host.querySelectorAll('.wf-rerun').forEach(btn => {
      btn.onclick = () => {
        const threadId = btn.dataset.thread;
        const route = btn.dataset.route;
        const threadInput = document.getElementById('wfThreadId');
        const routeInput = document.getElementById('wfRoute');
        if (threadInput && threadId) threadInput.value = threadId;
        if (routeInput) routeInput.value = route || '';
      };
    });
  }

  async function executeWorkflowRun(threadId, wfName, prompt, route) {
    const wfLog = document.getElementById('wfLog');
    wfLog.innerHTML = '';
    if (route) {
      localStorage.setItem(`rc_workflow_route_${wfName}`, route);
    } else {
      localStorage.removeItem(`rc_workflow_route_${wfName}`);
    }
    const runKey = `rc_workflow_runs_${wfName}`;
    pushHistory(runKey, {
      id: `${wfName}:${Date.now()}`,
      workflow: wfName,
      thread_id: threadId,
      route: route || '',
      ran_at: new Date().toISOString(),
    }, 10, 'id');
    renderWorkflowHistory(runKey);
    await sendWorkflowStream(threadId, wfName, prompt, route || null, (event, data) => {
      if (event === 'agent_event') {
        const payload = safeJson(data);
        wfLog.insertAdjacentHTML('beforeend', `<div>agent_event: ${escapeHtml(payload.agent_name || '')}</div>`);
      } else if (event === 'group_complete') {
        const payload = safeJson(data);
        wfLog.insertAdjacentHTML('beforeend', `<div>group_complete: ${payload.group}</div>`);
      } else if (event === 'repivot_applied') {
        const payload = safeJson(data);
        wfLog.insertAdjacentHTML('beforeend', `<div>repivot: ${escapeHtml(payload.original_artifact_id || '')} \u2192 ${escapeHtml(payload.new_artifact_id || '')} (${escapeHtml(payload.new_filename || '')})</div>`);
      } else if (event === 'fan_out_started') {
        const payload = safeJson(data);
        const threads = (payload.child_thread_ids || []).join(', ');
        wfLog.insertAdjacentHTML('beforeend', `<div>fan_out_started: ${payload.child_count} children (threads: ${escapeHtml(threads)})</div>`);
      } else if (event === 'fan_out_complete') {
        const payload = safeJson(data);
        wfLog.insertAdjacentHTML('beforeend', `<div>fan_out_complete: ${payload.completed} ok / ${payload.failed} failed</div>`);
      } else if (event === 'workflow_complete') {
        const payload = safeJson(data);
        wfLog.insertAdjacentHTML('beforeend', `<div>workflow_complete: ${escapeHtml(payload.workflow_name || '')}</div>`);
      } else if (event === 'error') {
        wfLog.insertAdjacentHTML('beforeend', `<div class="error">error: ${escapeHtml(data)}</div>`);
      } else {
        wfLog.insertAdjacentHTML('beforeend', `<div>${escapeHtml(event)}: ${escapeHtml(data)}</div>`);
      }
    });
  }

  async function sendWorkflowStream(threadId, workflowName, prompt, routeOverride, onEvent) {
    const headers = { 'Content-Type': 'application/json', 'Accept': 'text/event-stream' };
    if (state.apiKey) headers['Authorization'] = `Bearer ${state.apiKey}`;
    const payload = { workflow_name: workflowName, content: prompt || '' };
    if (routeOverride) payload.route = routeOverride;
    const res = await fetch(`${API_BASE}/threads/${threadId}/workflow`, {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
    });
    if (!res.ok) {
      if (res.status === 401) {
        setApiKey('');
        throw new Error('Session expired. Please log in again.');
      }
      const text = await res.text();
      throw new Error(text || `${res.status} ${res.statusText}`);
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const parts = buffer.split('\n\n');
      buffer = parts.pop();
      for (const part of parts) {
        const lines = part.split('\n');
        let event = 'message';
        const dataLines = [];
        for (const line of lines) {
          if (line.startsWith('event:')) event = line.slice(6).trim();
          if (line.startsWith('data:')) dataLines.push(line.slice(5).trim());
        }
        const data = dataLines.join('\n');
        onEvent(event, data);
      }
    }
  }

  async function renderProjects() {
    let rows = [];
    try {
      rows = await apiFetch('/projects');
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }
    // Populate NDA cache from project list
    for (const r of rows) _ndaProjectCache[r.id] = !!r.nda;

    renderShell(`
      <div class="card">
        <h3>Projects</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="projectName" class="input" placeholder="New project name" />
          <button id="createProject" class="button">Create</button>
        </div>
        <div class="row" style="margin-bottom:10px;">
          <input id="projectFilter" class="input" placeholder="Filter by id or name" />
          <div class="muted" id="projectCount"></div>
        </div>
        <div class="pagination" id="projectPager"></div>
        <table class="table">
          <thead><tr><th>Name</th><th>ID</th><th>Created</th><th></th></tr></thead>
          <tbody id="projectTableBody"></tbody>
        </table>
      </div>
    `);

    const tableBody = document.getElementById('projectTableBody');
    const projectCount = document.getElementById('projectCount');
    const filterInput = document.getElementById('projectFilter');
    const pageSize = 10;
    let page = 1;

    const renderRows = (items) => items.map(r => `
      <tr>
        <td><a href="#/projects/${r.id}">${escapeHtml(r.name || '(untitled)')}</a> ${r.nda ? '<span class="badge-nda">NDA</span>' : ''}</td>
        <td class="muted">${r.id}</td>
        <td>${new Date(r.created_at).toLocaleString()}</td>
        <td><button class="button secondary del-project" data-id="${r.id}" data-name="${escapeHtml(r.name || '')}" style="color:#c00;font-size:0.8em;padding:2px 8px;">Delete</button></td>
      </tr>
    `).join('');

    const renderPage = (items) => {
      const totalPages = Math.max(1, Math.ceil(items.length / pageSize));
      if (page > totalPages) page = totalPages;
      const start = (page - 1) * pageSize;
      const slice = items.slice(start, start + pageSize);
      tableBody.innerHTML = renderRows(slice);
      projectCount.textContent = `${items.length} total · page ${page}/${totalPages}`;
      const pager = document.getElementById('projectPager');
      pager.innerHTML = `
        <button class="button secondary" id="projPrev" ${page <= 1 ? 'disabled' : ''}>Prev</button>
        <button class="button secondary" id="projNext" ${page >= totalPages ? 'disabled' : ''}>Next</button>
      `;
      document.getElementById('projPrev').onclick = () => { page -= 1; renderPage(items); };
      document.getElementById('projNext').onclick = () => { page += 1; renderPage(items); };
    };

    const applyFilter = () => {
      const q = filterInput.value.trim().toLowerCase();
      const filtered = q
        ? rows.filter(r => {
            const hay = `${r.id} ${r.name || ''}`.toLowerCase();
            return hay.includes(q);
          })
        : rows;
      page = 1;
      renderPage(filtered);
    };

    tableBody.addEventListener('click', async (e) => {
      const btn = e.target.closest('.del-project');
      if (!btn) return;
      const id = btn.dataset.id;
      const name = btn.dataset.name;
      if (!confirm(`Delete project "${name}"?\n\nThis will permanently delete ALL conversations, artifacts, and data in this project. This cannot be undone.`)) return;
      try {
        await apiFetch(`/projects/${id}`, { method: 'DELETE' });
        rows = rows.filter(r => r.id !== id);
        applyFilter();
      } catch (e) {
        alert(`Delete failed: ${e.message}`);
      }
    });

    filterInput.oninput = applyFilter;
    applyFilter();

    document.getElementById('createProject').onclick = async () => {
      const name = document.getElementById('projectName').value.trim();
      if (!name) return;
      try {
        await apiFetch('/projects', {
          method: 'POST',
          body: JSON.stringify({ name })
        });
        await renderProjects();
      } catch (e) {
        alert(e.message);
      }
    };
  }

  async function renderPlugins() {
    let plugins = [];
    try {
      plugins = await apiFetch('/plugins');
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const rows = plugins.map(p => {
      const toolList = (p.tools || []).map(t => escapeHtml(t)).join(', ') || '<span class="muted">none</span>';
      const agentList = (p.agents || []).map(a => `<a href="#/agents/${a}">${escapeHtml(a)}</a>`).join(', ') || '<span class="muted">none</span>';
      const wfList = (p.workflows || []).map(w => `<a href="#/workflows/${w}">${escapeHtml(w)}</a>`).join(', ') || '<span class="muted">none</span>';
      return `
        <tr>
          <td><strong>${escapeHtml(p.name)}</strong></td>
          <td>${toolList}</td>
          <td>${agentList}</td>
          <td>${wfList}</td>
        </tr>
      `;
    }).join('');

    renderShell(`
      <div class="card">
        <h3>Loaded Plugins</h3>
        <p class="muted">Shows which plugins provided each tool, agent, and workflow.</p>
        <table class="table">
          <thead><tr><th>Source</th><th>Tools</th><th>Agents</th><th>Workflows</th></tr></thead>
          <tbody>${rows}</tbody>
        </table>
      </div>
    `);
  }

  async function renderTools() {
    let tools = [];
    try {
      tools = await apiFetch('/tools');
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const sources = [...new Set(tools.map(t => t.source || 'unknown'))].sort();
    const filterOptions = sources.map(s => `<option value="${escapeHtml(s)}">${escapeHtml(s)}</option>`).join('');

    function buildToolRows(filter) {
      return tools
        .filter(t => !filter || (t.source || 'unknown') === filter)
        .map(t => `
          <tr>
            <td>${escapeHtml(t.name)}</td>
            <td>${t.version}</td>
            <td>${escapeHtml(t.source || 'unknown')}</td>
            <td>${escapeHtml(t.description || '')}</td>
          </tr>
        `).join('');
    }

    const options = tools
      .map(t => `<option value="${escapeHtml(t.name)}">${escapeHtml(t.name)}</option>`)
      .join('');

    renderShell(`
      <div class="card">
        <h3>Tools</h3>
        <div style="margin-bottom:10px;">
          <select id="toolSourceFilter" class="input" style="width:auto;">
            <option value="">All sources</option>
            ${filterOptions}
          </select>
        </div>
        <table class="table">
          <thead><tr><th>Name</th><th>Version</th><th>Source</th><th>Description</th></tr></thead>
          <tbody id="toolTableBody">${buildToolRows('')}</tbody>
        </table>
      </div>
      <div class="card">
        <h4>Run Tool</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="toolName" class="input" list="toolList" placeholder="tool name" />
          <datalist id="toolList">${options}</datalist>
          <input id="toolProjectId" class="input" placeholder="project UUID" />
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="toolInput" class="input" placeholder="input JSON">{}</textarea>
        </div>
        <button id="toolRun" class="button">Run</button>
        <div id="toolRunErr" class="error" style="margin-top:10px;"></div>
        <div class="card" style="margin-top:10px;">
          <h4>Output</h4>
          <pre id="toolOutput" class="muted"></pre>
        </div>
      </div>
    `);

    document.getElementById('toolSourceFilter').onchange = (e) => {
      document.getElementById('toolTableBody').innerHTML = buildToolRows(e.target.value);
    };

    document.getElementById('toolRun').onclick = async () => {
      const name = document.getElementById('toolName').value.trim();
      const projectId = document.getElementById('toolProjectId').value.trim();
      const inputRaw = document.getElementById('toolInput').value.trim() || '{}';
      if (!name || !projectId) return;
      let inputJson;
      try {
        inputJson = JSON.parse(inputRaw);
      } catch (e) {
        document.getElementById('toolRunErr').textContent = 'Input JSON is invalid.';
        return;
      }
      try {
        const result = await apiFetch(`/tools/${name}/run`, {
          method: 'POST',
          body: JSON.stringify({ project_id: projectId, input: inputJson }),
        });
        const output = {
          status: result.status,
          produced_artifacts: result.produced_artifacts || [],
          output: result.output,
        };
        document.getElementById('toolOutput').textContent = JSON.stringify(output, null, 2);
        document.getElementById('toolRunErr').textContent = '';
      } catch (e) {
        document.getElementById('toolRunErr').textContent = e.message;
      }
    };
  }

  async function renderAgents() {
    let agents = [];
    let routes = [];
    try {
      [agents, routes] = await Promise.all([apiFetch('/agents'), fetchRoutes()]);
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const routeOptions = routes
      .map(r => `<option value="${escapeHtml(r)}">${escapeHtml(r)}</option>`)
      .join('');

    const agentSources = [...new Set(agents.map(a => a.source_plugin || 'user'))].sort();
    const agentFilterOpts = agentSources.map(s => `<option value="${escapeHtml(s)}">${escapeHtml(s)}</option>`).join('');

    function buildAgentRows(filter) {
      return agents
        .filter(a => !filter || (a.source_plugin || 'user') === filter)
        .map(a => `
          <tr>
            <td><a href="#/agents/${a.name}">${escapeHtml(a.name)}</a></td>
            <td>${escapeHtml(a.source_plugin || 'user')}</td>
            <td>${escapeHtml(a.default_route || '')}</td>
            <td>${new Date(a.updated_at).toLocaleString()}</td>
          </tr>
        `).join('');
    }

    renderShell(`
      <div class="card">
        <h3>Agents</h3>
        <p class="muted">Create and manage agent profiles (admin only for writes).</p>
        <div style="margin-bottom:10px;">
          <select id="agentSourceFilter" class="input" style="width:auto;">
            <option value="">All sources</option>
            ${agentFilterOpts}
          </select>
        </div>
        <table class="table">
          <thead><tr><th>Name</th><th>Source</th><th>Route</th><th>Updated</th></tr></thead>
          <tbody id="agentTableBody">${buildAgentRows('')}</tbody>
        </table>
      </div>
      <div class="card">
        <h4>Create Agent</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="agentName" class="input" placeholder="name" />
          <input id="agentRoute" class="input" list="routeList" placeholder="default_route (auto)" />
          <datalist id="routeList">${routeOptions}</datalist>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="agentPrompt" class="input" placeholder="system prompt"></textarea>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="agentTools" class="input" placeholder="allowed tools (one per line or comma-separated)"></textarea>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="agentMeta" class="input" placeholder="metadata JSON (optional)"></textarea>
        </div>
        <button id="agentCreate" class="button">Create</button>
      </div>
    `);

    document.getElementById('agentSourceFilter').onchange = (e) => {
      document.getElementById('agentTableBody').innerHTML = buildAgentRows(e.target.value);
    };

    document.getElementById('agentCreate').onclick = async () => {
      const name = document.getElementById('agentName').value.trim();
      const defaultRoute = document.getElementById('agentRoute').value.trim() || 'auto';
      const systemPrompt = document.getElementById('agentPrompt').value.trim();
      const tools = parseAllowedTools(document.getElementById('agentTools').value);
      const metaRaw = document.getElementById('agentMeta').value.trim();
      if (!name || !systemPrompt || tools.length === 0) return;
      let metadata = {};
      if (metaRaw) {
        try {
          metadata = JSON.parse(metaRaw);
        } catch (e) {
          alert('metadata JSON is invalid');
          return;
        }
      }
      try {
        await apiFetch('/agents', {
          method: 'POST',
          body: JSON.stringify({
            name,
            system_prompt: systemPrompt,
            allowed_tools: tools,
            default_route: defaultRoute,
            metadata,
          }),
        });
        navigate(`#/agents/${name}`);
      } catch (e) {
        alert(e.message);
      }
    };
  }

  async function renderAgentDetail(name) {
    let agent;
    let routes = [];
    try {
      [agent, routes] = await Promise.all([apiFetch(`/agents/${name}`), fetchRoutes()]);
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const routeOptions = routes
      .map(r => `<option value="${escapeHtml(r)}">${escapeHtml(r)}</option>`)
      .join('');

    renderShell(`
      <div class="card">
        <h3>Agent ${escapeHtml(agent.name)}</h3>
        <p class="muted">${agent.is_builtin ? 'Builtin agent (read-only name)' : 'Custom agent'}</p>
        <div class="row" style="margin-bottom:10px;">
          <input id="agentEditName" class="input" placeholder="name" value="${escapeHtml(agent.name)}" ${agent.is_builtin ? 'disabled' : ''} />
          <input id="agentEditRoute" class="input" list="routeList" placeholder="default_route" value="${escapeHtml(agent.default_route || '')}" />
          <datalist id="routeList">${routeOptions}</datalist>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="agentEditPrompt" class="input" placeholder="system prompt">${escapeHtml(agent.system_prompt || '')}</textarea>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="agentEditTools" class="input" placeholder="allowed tools (one per line or comma-separated)">${escapeHtml(formatAllowedTools(agent.allowed_tools))}</textarea>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="agentEditMeta" class="input" placeholder="metadata JSON (optional)">${escapeHtml(JSON.stringify(agent.metadata || {}, null, 2))}</textarea>
        </div>
        <div class="row">
          <button id="agentUpdate" class="button">Update</button>
          <button id="agentDelete" class="button secondary">Delete</button>
        </div>
      </div>
    `);

    document.getElementById('agentUpdate').onclick = async () => {
      const systemPrompt = document.getElementById('agentEditPrompt').value.trim();
      const defaultRoute = document.getElementById('agentEditRoute').value.trim() || 'auto';
      const tools = parseAllowedTools(document.getElementById('agentEditTools').value);
      const metaRaw = document.getElementById('agentEditMeta').value.trim();
      if (!systemPrompt || tools.length === 0) return;
      let metadata = {};
      if (metaRaw) {
        try {
          metadata = JSON.parse(metaRaw);
        } catch (e) {
          alert('metadata JSON is invalid');
          return;
        }
      }
      try {
        await apiFetch(`/agents/${agent.name}`, {
          method: 'PUT',
          body: JSON.stringify({
            system_prompt: systemPrompt,
            allowed_tools: tools,
            default_route: defaultRoute,
            metadata,
          }),
        });
        navigate(`#/agents/${agent.name}`);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('agentDelete').onclick = async () => {
      if (agent.is_builtin) {
        alert('Cannot delete builtin agent.');
        return;
      }
      if (!confirm(`Delete agent '${agent.name}'?`)) return;
      try {
        await apiFetch(`/agents/${agent.name}`, { method: 'DELETE' });
        navigate('#/agents');
      } catch (e) {
        alert(e.message);
      }
    };
  }

  // Quick-launch dialog for analyzing a specific sample
  function showAnalyzeDialog(projectId, artifactId, filename, sha256) {
    // Remove existing dialog if any
    const existing = document.getElementById('analyzeDialog');
    if (existing) existing.remove();

    const overlay = document.createElement('div');
    overlay.id = 'analyzeDialog';
    overlay.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.5);z-index:100;display:flex;align-items:center;justify-content:center;';
    overlay.innerHTML = `
      <div style="background:var(--bg, #1e1e1e);border:1px solid #444;border-radius:8px;padding:20px;max-width:400px;width:90%;">
        <h3 style="margin-top:0;">Analyze: ${escapeHtml(filename)}</h3>
        <div style="margin-bottom:12px;">
          <label class="muted" style="display:block;margin-bottom:4px;">Mode</label>
          <select id="adMode" class="input">
            <option value="workflow">Workflow</option>
            <option value="agent">Agent Chat</option>
            <option value="thinking">Thinking Thread</option>
          </select>
        </div>
        <div id="adWorkflowRow" style="margin-bottom:12px;">
          <label class="muted" style="display:block;margin-bottom:4px;">Workflow</label>
          <select id="adWorkflow" class="input"><option value="">Loading...</option></select>
        </div>
        <div id="adAgentRow" style="display:none;margin-bottom:12px;">
          <label class="muted" style="display:block;margin-bottom:4px;">Agent</label>
          <select id="adAgent" class="input"><option value="">Loading...</option></select>
        </div>
        <div style="margin-bottom:12px;">
          <label class="muted" style="display:block;margin-bottom:4px;">Title (optional)</label>
          <input id="adTitle" class="input" value="${escapeHtml(filename)}" />
        </div>
        <div class="row" style="justify-content:flex-end;gap:8px;">
          <button id="adCancel" class="button secondary">Cancel</button>
          <button id="adGo" class="button">Go</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    // Close on backdrop click
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
    document.getElementById('adCancel').onclick = () => overlay.remove();

    // Load workflows and agents
    Promise.all([
      apiFetch('/workflows').catch(() => []),
      apiFetch('/agents').then(list => list.map(a => a.name)).catch(() => []),
    ]).then(([wfs, agentNames]) => {
      const wfSel = document.getElementById('adWorkflow');
      if (wfSel) {
        wfSel.innerHTML = wfs.map(w =>
          `<option value="${escapeHtml(w.name)}">${escapeHtml(w.name)}</option>`
        ).join('');
      }
      const agentSel = document.getElementById('adAgent');
      if (agentSel) {
        agentSel.innerHTML = [
          '<option value="default">default</option>',
          ...agentNames.filter(n => n !== 'default').map(n =>
            `<option value="${escapeHtml(n)}">${escapeHtml(n)}</option>`
          ),
        ].join('');
      }
    });

    // Mode toggle
    document.getElementById('adMode').onchange = () => {
      const m = document.getElementById('adMode').value;
      document.getElementById('adWorkflowRow').style.display = m === 'workflow' ? '' : 'none';
      document.getElementById('adAgentRow').style.display = m !== 'workflow' ? '' : 'none';
    };

    document.getElementById('adGo').onclick = async () => {
      // Read all values before removing the overlay
      const mode = document.getElementById('adMode').value;
      const title = document.getElementById('adTitle').value.trim() || filename;
      const wfName = document.getElementById('adWorkflow')?.value || '';
      const agent = document.getElementById('adAgent')?.value?.trim() || '';
      overlay.remove();

      try {
        if (mode === 'workflow') {
          const thread = await apiFetch(`/projects/${projectId}/threads`, {
            method: 'POST',
            body: JSON.stringify({ agent_name: 'default', title: `${wfName}: ${title}`, thread_type: 'workflow', target_artifact_id: artifactId }),
          });
          location.hash = `#/thread/${thread.id}`;
        } else if (mode === 'thinking') {
          const thinkAgent = agent || 'default';
          const thread = await apiFetch(`/projects/${projectId}/threads`, {
            method: 'POST',
            body: JSON.stringify({ agent_name: thinkAgent, title: `Think: ${title}`, thread_type: 'thinking', target_artifact_id: artifactId }),
          });
          // Pre-fill analysis goal so "Run Analysis" panel shows a useful default
          localStorage.setItem(`rc_thinking_goal_${thread.id}`, `Analyze sample "${filename}" (sha256: ${sha256}). Perform a full analysis using your available tools.`);
          location.hash = `#/thread/${thread.id}`;
        } else {
          const agentName = agent || 'default';
          const thread = await apiFetch(`/projects/${projectId}/threads`, {
            method: 'POST',
            body: JSON.stringify({ agent_name: agentName, title, target_artifact_id: artifactId }),
          });
          location.hash = `#/thread/${thread.id}`;
        }
      } catch (e) {
        alert(e.message);
      }
    };
  }

  async function renderProjectDetail(projectId) {
    let artifacts = [];
    let members = [];
    let hooks = [];
    let costData = null;
    let projectData = null;
    try {
      [artifacts, members, hooks, costData, projectData] = await Promise.all([
        apiFetch(`/projects/${projectId}/artifacts`),
        apiFetch(`/projects/${projectId}/members`).catch(() => []),
        apiFetch(`/projects/${projectId}/hooks`).catch(() => []),
        apiFetch(`/projects/${projectId}/cost`).catch(() => null),
        apiFetch(`/projects/${projectId}`).catch(() => null),
      ]);
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const isNda = projectData ? !!projectData.nda : false;
    if (projectData) _ndaProjectCache[projectId] = isNda;

    // Determine if current user can manage members (owner or manager)
    const canManage = members.some(m =>
      m.role === 'owner' || m.role === 'manager'
    );
    const isPublic = members.some(m => m.user_id === '@all');

    const memberRows = members.map(m => `
      <tr>
        <td>${escapeHtml(m.user_id)}</td>
        <td>${escapeHtml(m.display_name || '-')}</td>
        <td>${escapeHtml(m.role)}</td>
        <td>${canManage && m.role !== 'owner' ? `<button class="button secondary remove-member" data-uid="${escapeHtml(m.user_id)}">Remove</button>` : ''}</td>
      </tr>
    `).join('');

    const addMemberControls = canManage ? `
      <div class="row" style="margin-top:10px;">
        <input id="memberUserId" class="input" placeholder="User UUID or @all" style="width:320px;" />
        <select id="memberRole" class="input" style="width:140px;">
          <option value="viewer">viewer</option>
          <option value="collaborator">collaborator</option>
          <option value="manager">manager</option>
        </select>
        <button id="addMemberBtn" class="button">Add Member</button>
      </div>
      <div class="row" style="margin-top:6px;">
        <label style="display:flex;align-items:center;gap:6px;">
          <input id="publicToggle" type="checkbox" ${isPublic ? 'checked' : ''} />
          Make publicly visible (@all viewer)
        </label>
      </div>
    ` : '';

    renderShell(`
      <div class="card">
        <h3>Project ${projectId} ${isNda ? '<span class="badge-nda">NDA</span>' : ''}</h3>
        <div class="row" style="margin-bottom:10px;">
          <button id="viewThreads" class="button">Conversations</button>
          <button id="copyProjectId" class="button secondary">Copy Project ID</button>
          <button id="cleanGenerated" class="button secondary">Clean Generated Artifacts</button>
          <button id="deleteProjectBtn" class="button secondary" style="color:#c00;">Delete Project</button>
        </div>
        <div class="row" style="margin-bottom:10px;">
          <input id="uploadFile" class="input" type="file" multiple />
          <button id="uploadBtn" class="button">Upload Samples</button>
        </div>
        <div id="uploadQueue" class="muted" style="margin-bottom:10px;"></div>
        <div class="row" style="margin-bottom:10px;">
          <input id="artifactFilter" class="input" placeholder="Filter by id, filename, or sha256" />
          <div class="muted" id="artifactCount"></div>
        </div>
        <h4>Samples (uploaded)</h4>
        <table class="table">
          <thead><tr><th>Filename</th><th>Description</th><th>SHA256</th><th>ID</th><th></th></tr></thead>
          <tbody id="sampleTableBody"></tbody>
        </table>
        <div class="muted" id="sampleCount" style="margin-top:4px;"></div>
        <details style="margin-top:12px;">
          <summary style="cursor:pointer;font-weight:bold;">Generated Artifacts <span class="muted" id="generatedCount"></span></summary>
          <table class="table" style="margin-top:6px;">
            <thead><tr><th>Filename</th><th>Description</th><th>SHA256</th><th>ID</th><th></th></tr></thead>
            <tbody id="generatedTableBody"></tbody>
          </table>
        </details>
      </div>
      <div class="card">
        <h4>Members</h4>
        <table class="table">
          <thead><tr><th>User</th><th>Display Name</th><th>Role</th><th></th></tr></thead>
          <tbody id="memberTableBody">${memberRows}</tbody>
        </table>
        <div id="memberError" class="error" style="margin-top:6px;"></div>
        ${addMemberControls}
      </div>
      <div class="card">
        <h4>Hooks</h4>
        <table class="table">
          <thead><tr><th>Name</th><th>Event</th><th>Target</th><th>Enabled</th><th></th></tr></thead>
          <tbody id="hookTableBody"></tbody>
        </table>
        <div class="muted" id="hookCount" style="margin-top:4px;"></div>
        <div class="row" style="margin-top:10px;">
          <button id="manageHooksBtn" class="button secondary">Manage Hooks</button>
          <button id="createHookBtn" class="button">Add Hook</button>
        </div>
        <div id="hookError" class="error" style="margin-top:6px;"></div>
      </div>
      ${renderCostCard(costData)}
    `);

    const sampleBody = document.getElementById('sampleTableBody');
    const generatedBody = document.getElementById('generatedTableBody');
    const sampleCountEl = document.getElementById('sampleCount');
    const generatedCountEl = document.getElementById('generatedCount');
    const artifactCount = document.getElementById('artifactCount');
    const filterInput = document.getElementById('artifactFilter');

    // Split: uploaded samples vs tool-generated artifacts
    const samples = artifacts.filter(a => !a.source_tool_run_id);
    const generated = artifacts.filter(a => !!a.source_tool_run_id);

    const truncDesc = (d) => d && d.length > 40 ? escapeHtml(d.substring(0, 40)) + '...' : escapeHtml(d || '');
    const renderRows = (items, showAnalyze) => items.map(a => `
      <tr>
        <td><a href="#/projects/${projectId}/artifacts/${a.id}">${escapeHtml(a.filename)}</a></td>
        <td class="muted">${truncDesc(a.description)}</td>
        <td class="muted">${escapeHtml(a.sha256)}</td>
        <td class="muted">${a.id}</td>
        <td>
          <button class="button secondary" data-dl="${a.id}" data-fn="${escapeHtml(a.filename)}">Download</button>
          ${showAnalyze ? `<button class="button" data-analyze="${a.id}" data-afn="${escapeHtml(a.filename)}" data-sha="${escapeHtml(a.sha256)}" style="margin-left:4px;">Analyze</button>` : ''}
        </td>
      </tr>
    `).join('');

    const applyFilter = () => {
      const q = filterInput.value.trim().toLowerCase();
      const filterFn = (a) => {
        const hay = `${a.id} ${a.filename || ''} ${a.sha256 || ''}`.toLowerCase();
        return hay.includes(q);
      };
      const filteredSamples = q ? samples.filter(filterFn) : samples;
      const filteredGenerated = q ? generated.filter(filterFn) : generated;
      sampleBody.innerHTML = renderRows(filteredSamples, true);
      generatedBody.innerHTML = renderRows(filteredGenerated, false);
      sampleCountEl.textContent = `${filteredSamples.length} sample(s)`;
      generatedCountEl.textContent = `(${filteredGenerated.length})`;
      artifactCount.textContent = `${filteredSamples.length + filteredGenerated.length}/${artifacts.length}`;
      document.querySelectorAll('[data-dl]').forEach(btn => {
        btn.onclick = async () => {
          try {
            await downloadArtifact(btn.dataset.dl, btn.dataset.fn);
          } catch (e) {
            alert(e.message);
          }
        };
      });
      // Wire "Analyze" buttons — show quick-launch dialog
      document.querySelectorAll('[data-analyze]').forEach(btn => {
        btn.onclick = () => showAnalyzeDialog(projectId, btn.dataset.analyze, btn.dataset.afn, btn.dataset.sha);
      });
    };

    filterInput.oninput = applyFilter;
    applyFilter();
    applyNdaTint(isNda);

    document.getElementById('viewThreads').onclick = () => navigate(`#/threads/${projectId}`);
    document.getElementById('copyProjectId').onclick = async () => {
      await copyToClipboard(projectId);
      alert('Copied project ID.');
    };
    document.getElementById('cleanGenerated').onclick = async () => {
      if (!confirm('Delete all generated artifacts (function lists, decompiled output, etc.)?\n\nUploaded samples will not be affected.')) return;
      try {
        const result = await apiFetch(`/projects/${projectId}/artifacts/generated`, { method: 'DELETE' });
        alert(`Deleted ${result.deleted} generated artifact(s).`);
        await renderProjectDetail(projectId);
      } catch (e) {
        alert(`Failed: ${e.message}`);
      }
    };
    document.getElementById('deleteProjectBtn').onclick = async () => {
      if (!confirm('DELETE THIS ENTIRE PROJECT?\n\nThis will permanently delete ALL conversations, ALL artifacts, ALL data associated with this project.\n\nThis action CANNOT be undone.')) return;
      try {
        await apiFetch(`/projects/${projectId}`, { method: 'DELETE' });
        navigate('#/projects');
      } catch (e) {
        alert(`Delete failed: ${e.message}`);
      }
    };

    document.getElementById('uploadBtn').onclick = async () => {
      const fileInput = document.getElementById('uploadFile');
      if (!fileInput.files || fileInput.files.length === 0) return;
      const queue = document.getElementById('uploadQueue');
      queue.innerHTML = '';
      const files = Array.from(fileInput.files);
      for (const file of files) {
        const row = document.createElement('div');
        row.className = 'row';
        row.innerHTML = `
          <div>${escapeHtml(file.name)}</div>
          <div class="muted upload-status">0%</div>
          <button class="button secondary">Cancel</button>
        `;
        queue.appendChild(row);
        const status = row.querySelector('.upload-status');
        const cancelBtn = row.querySelector('button');
        const uploader = apiUploadWithProgress(`/projects/${projectId}/artifacts`, file, (pct) => {
          status.textContent = `${pct}%`;
        });
        cancelBtn.onclick = () => {
          uploader.abort();
          status.textContent = 'cancelled';
          cancelBtn.disabled = true;
        };
        try {
          await uploader.promise;
          status.textContent = 'done';
        } catch (e) {
          status.textContent = `error: ${e.message}`;
        }
      }
      await renderProjectDetail(projectId);
    };

    // Member management handlers
    document.querySelectorAll('.remove-member').forEach(btn => {
      btn.onclick = async () => {
        const uid = btn.dataset.uid;
        if (!confirm(`Remove member ${uid}?`)) return;
        try {
          await apiFetch(`/projects/${projectId}/members/${encodeURIComponent(uid)}`, { method: 'DELETE' });
          await renderProjectDetail(projectId);
        } catch (e) {
          const errEl = document.getElementById('memberError');
          if (errEl) errEl.textContent = e.message;
        }
      };
    });

    const addMemberBtn = document.getElementById('addMemberBtn');
    if (addMemberBtn) {
      addMemberBtn.onclick = async () => {
        const userIdInput = document.getElementById('memberUserId');
        const roleSelect = document.getElementById('memberRole');
        const userId = userIdInput.value.trim();
        if (!userId) return;
        try {
          await apiFetch(`/projects/${projectId}/members`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ user_id: userId, role: roleSelect.value }),
          });
          await renderProjectDetail(projectId);
        } catch (e) {
          const errEl = document.getElementById('memberError');
          if (errEl) errEl.textContent = e.message;
        }
      };
    }

    const publicToggle = document.getElementById('publicToggle');
    if (publicToggle) {
      publicToggle.onchange = async () => {
        try {
          if (publicToggle.checked) {
            await apiFetch(`/projects/${projectId}/members`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ user_id: '@all', role: 'viewer' }),
            });
          } else {
            await apiFetch(`/projects/${projectId}/members/${encodeURIComponent('@all')}`, { method: 'DELETE' });
          }
          await renderProjectDetail(projectId);
        } catch (e) {
          const errEl = document.getElementById('memberError');
          if (errEl) errEl.textContent = e.message;
          publicToggle.checked = !publicToggle.checked;
        }
      };
    }

    // Hooks section
    const hookBody = document.getElementById('hookTableBody');
    const hookCount = document.getElementById('hookCount');
    if (hookBody) {
      hookBody.innerHTML = hooks.map(h => {
        const target = h.workflow_name
          ? `wf:${escapeHtml(h.workflow_name)}`
          : h.agent_name ? `agent:${escapeHtml(h.agent_name)}` : '';
        const interval = h.tick_interval_minutes ? ` (${h.tick_interval_minutes}m)` : '';
        return `<tr>
          <td><a href="#/hooks/${projectId}/${h.id}">${escapeHtml(h.name)}</a></td>
          <td>${escapeHtml(h.event_type)}${interval}</td>
          <td>${target}</td>
          <td>${h.enabled ? 'yes' : 'no'}</td>
          <td>
            <button class="button secondary toggle-hook" data-hid="${h.id}" data-enabled="${h.enabled}">${h.enabled ? 'Disable' : 'Enable'}</button>
            <button class="button secondary delete-hook" data-hid="${h.id}">Delete</button>
          </td>
        </tr>`;
      }).join('');
      hookCount.textContent = `${hooks.length} hook(s)`;
    }

    document.querySelectorAll('.toggle-hook').forEach(btn => {
      btn.onclick = async () => {
        const hid = btn.dataset.hid;
        const nowEnabled = btn.dataset.enabled === 'true';
        try {
          await apiFetch(`/hooks/${hid}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ enabled: !nowEnabled }),
          });
          await renderProjectDetail(projectId);
        } catch (e) {
          const errEl = document.getElementById('hookError');
          if (errEl) errEl.textContent = e.message;
        }
      };
    });

    document.querySelectorAll('.delete-hook').forEach(btn => {
      btn.onclick = async () => {
        const hid = btn.dataset.hid;
        if (!confirm('Delete this hook?')) return;
        try {
          await apiFetch(`/hooks/${hid}`, { method: 'DELETE' });
          await renderProjectDetail(projectId);
        } catch (e) {
          const errEl = document.getElementById('hookError');
          if (errEl) errEl.textContent = e.message;
        }
      };
    });

    document.getElementById('manageHooksBtn').onclick = () => navigate(`#/hooks/${projectId}`);
    document.getElementById('createHookBtn').onclick = () => navigate(`#/hooks/${projectId}/new`);

  }

  // --- Hooks pages ---

  async function renderHooks(projectId) {
    let hooks = [];
    let workflows = [];
    let agents = [];
    try {
      [hooks, workflows, agents] = await Promise.all([
        apiFetch(`/projects/${projectId}/hooks`),
        apiFetch('/workflows').catch(() => []),
        apiFetch('/agents').catch(() => []),
      ]);
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }
    checkProjectNda(projectId).then(nda => applyNdaTint(nda));

    const wfOptions = workflows.map(w =>
      `<option value="${escapeHtml(w.name)}">${escapeHtml(w.name)}</option>`
    ).join('');
    const agentOptions = agents.map(a =>
      `<option value="${escapeHtml(a.name)}">${escapeHtml(a.name)}</option>`
    ).join('');

    const hookRows = hooks.map(h => {
      const target = h.workflow_name
        ? `wf:${escapeHtml(h.workflow_name)}`
        : h.agent_name ? `agent:${escapeHtml(h.agent_name)}` : '';
      const interval = h.tick_interval_minutes ? ` (every ${h.tick_interval_minutes}m)` : '';
      const lastTick = h.last_tick_at ? new Date(h.last_tick_at).toLocaleString() : '-';
      return `<tr>
        <td><a href="#/hooks/${projectId}/${h.id}">${escapeHtml(h.name)}</a></td>
        <td>${escapeHtml(h.event_type)}${interval}</td>
        <td>${target}</td>
        <td>${h.enabled ? 'yes' : 'no'}</td>
        <td class="muted">${lastTick}</td>
        <td>
          <button class="button secondary toggle-hook" data-hid="${h.id}" data-enabled="${h.enabled}">${h.enabled ? 'Disable' : 'Enable'}</button>
          <button class="button secondary delete-hook" data-hid="${h.id}">Delete</button>
        </td>
      </tr>`;
    }).join('');

    renderShell(`
      <div class="card">
        <h3>Hooks for Project ${projectId}</h3>
        <p class="muted">Hooks automatically trigger workflows or agents in response to events.</p>
        <div class="row" style="margin-bottom:10px;">
          <a href="#/projects/${projectId}" class="button secondary">Back to Project</a>
        </div>
        <table class="table">
          <thead><tr><th>Name</th><th>Event</th><th>Target</th><th>Enabled</th><th>Last Tick</th><th></th></tr></thead>
          <tbody>${hookRows}</tbody>
        </table>
        <div class="muted" style="margin-top:4px;">${hooks.length} hook(s)</div>
        <div id="hookListError" class="error" style="margin-top:6px;"></div>
      </div>

      <div class="card">
        <h4>Create Hook</h4>
        <div class="row" style="margin-bottom:8px;">
          <input id="hookName" class="input" placeholder="Hook name" style="width:200px;" />
          <select id="hookEventType" class="input" style="width:180px;">
            <option value="artifact_uploaded">artifact_uploaded</option>
            <option value="tick">tick</option>
          </select>
        </div>
        <div class="row" style="margin-bottom:8px;">
          <select id="hookTargetMode" class="input" style="width:120px;">
            <option value="workflow">Workflow</option>
            <option value="agent">Agent</option>
          </select>
          <span id="hookTargetWfSpan" style="display:contents;">
            <select id="hookWorkflow" class="input">${wfOptions}</select>
          </span>
          <span id="hookTargetAgSpan" style="display:none;">
            <select id="hookAgent" class="input">${agentOptions}</select>
          </span>
        </div>
        <div class="row" style="margin-bottom:8px;">
          <textarea id="hookPrompt" class="input" rows="3" placeholder="Prompt template (use {{artifact_id}}, {{filename}}, {{sha256}}, {{project_id}}, {{project_name}}, {{hook_name}}, {{tick_count}})" style="flex:1;"></textarea>
        </div>
        <div class="row" id="hookTickRow" style="margin-bottom:8px;display:none;">
          <input id="hookInterval" class="input" type="number" min="1" placeholder="Tick interval (minutes)" style="width:220px;" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <input id="hookRoute" class="input" placeholder="Route override (optional)" style="width:260px;" />
        </div>
        <button id="hookCreateBtn" class="button">Create Hook</button>
        <div id="hookCreateError" class="error" style="margin-top:6px;"></div>
      </div>
    `);

    // Toggle/delete handlers
    document.querySelectorAll('.toggle-hook').forEach(btn => {
      btn.onclick = async () => {
        const hid = btn.dataset.hid;
        const nowEnabled = btn.dataset.enabled === 'true';
        try {
          await apiFetch(`/hooks/${hid}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ enabled: !nowEnabled }),
          });
          await renderHooks(projectId);
        } catch (e) {
          const errEl = document.getElementById('hookListError');
          if (errEl) errEl.textContent = e.message;
        }
      };
    });

    document.querySelectorAll('.delete-hook').forEach(btn => {
      btn.onclick = async () => {
        if (!confirm('Delete this hook?')) return;
        try {
          await apiFetch(`/hooks/${btn.dataset.hid}`, { method: 'DELETE' });
          await renderHooks(projectId);
        } catch (e) {
          const errEl = document.getElementById('hookListError');
          if (errEl) errEl.textContent = e.message;
        }
      };
    });

    // Target mode toggle
    const targetMode = document.getElementById('hookTargetMode');
    const wfSpan = document.getElementById('hookTargetWfSpan');
    const agSpan = document.getElementById('hookTargetAgSpan');
    targetMode.onchange = () => {
      wfSpan.style.display = targetMode.value === 'workflow' ? 'contents' : 'none';
      agSpan.style.display = targetMode.value === 'agent' ? 'contents' : 'none';
    };

    // Event type toggle — show tick interval field for tick events
    const eventType = document.getElementById('hookEventType');
    const tickRow = document.getElementById('hookTickRow');
    eventType.onchange = () => {
      tickRow.style.display = eventType.value === 'tick' ? '' : 'none';
    };
    tickRow.style.display = eventType.value === 'tick' ? '' : 'none';

    // Create handler
    document.getElementById('hookCreateBtn').onclick = async () => {
      const name = document.getElementById('hookName').value.trim();
      const evType = eventType.value;
      const mode = targetMode.value;
      const prompt = document.getElementById('hookPrompt').value.trim();
      const route = document.getElementById('hookRoute').value.trim() || undefined;
      const interval = document.getElementById('hookInterval').value ? parseInt(document.getElementById('hookInterval').value) : undefined;

      if (!name) { alert('Name is required.'); return; }
      if (!prompt) { alert('Prompt template is required.'); return; }
      if (evType === 'tick' && (!interval || interval < 1)) { alert('Tick interval is required and must be > 0.'); return; }

      const body = {
        name,
        event_type: evType,
        prompt_template: prompt,
      };
      if (mode === 'workflow') body.workflow_name = document.getElementById('hookWorkflow').value;
      else body.agent_name = document.getElementById('hookAgent').value;
      if (route) body.route_override = route;
      if (interval) body.tick_interval_minutes = interval;

      try {
        await apiFetch(`/projects/${projectId}/hooks`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        await renderHooks(projectId);
      } catch (e) {
        const errEl = document.getElementById('hookCreateError');
        if (errEl) errEl.textContent = e.message;
      }
    };
  }

  async function renderHookDetail(projectId, hookId) {
    let hook;
    try {
      hook = await apiFetch(`/hooks/${hookId}`);
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    checkProjectNda(projectId).then(nda => applyNdaTint(nda));
    const lastTick = hook.last_tick_at ? new Date(hook.last_tick_at).toLocaleString() : '-';
    const target = hook.workflow_name
      ? `Workflow: ${escapeHtml(hook.workflow_name)}`
      : hook.agent_name ? `Agent: ${escapeHtml(hook.agent_name)}` : '-';

    renderShell(`
      <div class="card">
        <h3>Hook: ${escapeHtml(hook.name)}</h3>
        <div class="row" style="margin-bottom:10px;">
          <a href="#/hooks/${projectId}" class="button secondary">Back to Hooks</a>
          <a href="#/projects/${projectId}" class="button secondary">Back to Project</a>
        </div>
        <table class="table">
          <tbody>
            <tr><td><strong>ID</strong></td><td>${hook.id}</td></tr>
            <tr><td><strong>Project</strong></td><td>${hook.project_id}</td></tr>
            <tr><td><strong>Event</strong></td><td>${escapeHtml(hook.event_type)}</td></tr>
            <tr><td><strong>Target</strong></td><td>${target}</td></tr>
            <tr><td><strong>Enabled</strong></td><td>${hook.enabled ? 'yes' : 'no'}</td></tr>
            <tr><td><strong>Route</strong></td><td>${hook.route_override ? escapeHtml(hook.route_override) : '-'}</td></tr>
            <tr><td><strong>Tick interval</strong></td><td>${hook.tick_interval_minutes ? hook.tick_interval_minutes + ' min' : '-'}</td></tr>
            <tr><td><strong>Last tick</strong></td><td>${lastTick}</td></tr>
            <tr><td><strong>Tick gen</strong></td><td>${hook.tick_generation}</td></tr>
            <tr><td><strong>Created</strong></td><td>${new Date(hook.created_at).toLocaleString()}</td></tr>
            <tr><td><strong>Updated</strong></td><td>${new Date(hook.updated_at).toLocaleString()}</td></tr>
          </tbody>
        </table>
      </div>

      <div class="card">
        <h4>Prompt Template</h4>
        <textarea id="hookPromptEdit" class="input" rows="5" style="width:100%;">${escapeHtml(hook.prompt_template)}</textarea>
        <div class="row" style="margin-top:8px;">
          <input id="hookRouteEdit" class="input" placeholder="Route override (optional)" value="${escapeHtml(hook.route_override || '')}" style="width:260px;" />
          ${hook.event_type === 'tick' ? `<input id="hookIntervalEdit" class="input" type="number" min="1" value="${hook.tick_interval_minutes || ''}" placeholder="Tick interval (min)" style="width:180px;" />` : ''}
        </div>
        <div class="row" style="margin-top:8px;">
          <button id="hookUpdateBtn" class="button">Save Changes</button>
          <button id="hookToggleBtn" class="button secondary">${hook.enabled ? 'Disable' : 'Enable'}</button>
          <button id="hookDeleteBtn" class="button secondary" style="color:var(--danger);">Delete</button>
        </div>
        <div id="hookDetailError" class="error" style="margin-top:6px;"></div>
        <div id="hookDetailSuccess" class="muted" style="margin-top:6px;"></div>
      </div>
    `);

    document.getElementById('hookUpdateBtn').onclick = async () => {
      const body = {};
      const promptVal = document.getElementById('hookPromptEdit').value.trim();
      if (promptVal && promptVal !== hook.prompt_template) body.prompt_template = promptVal;
      const routeVal = document.getElementById('hookRouteEdit').value.trim();
      if (routeVal !== (hook.route_override || '')) body.route_override = routeVal || null;
      const intervalEl = document.getElementById('hookIntervalEdit');
      if (intervalEl) {
        const iv = parseInt(intervalEl.value);
        if (iv > 0 && iv !== hook.tick_interval_minutes) body.tick_interval_minutes = iv;
      }
      if (Object.keys(body).length === 0) {
        document.getElementById('hookDetailSuccess').textContent = 'No changes.';
        return;
      }
      try {
        await apiFetch(`/hooks/${hookId}`, {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        document.getElementById('hookDetailSuccess').textContent = 'Saved.';
        document.getElementById('hookDetailError').textContent = '';
        await renderHookDetail(projectId, hookId);
      } catch (e) {
        document.getElementById('hookDetailError').textContent = e.message;
      }
    };

    document.getElementById('hookToggleBtn').onclick = async () => {
      try {
        await apiFetch(`/hooks/${hookId}`, {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ enabled: !hook.enabled }),
        });
        await renderHookDetail(projectId, hookId);
      } catch (e) {
        document.getElementById('hookDetailError').textContent = e.message;
      }
    };

    document.getElementById('hookDeleteBtn').onclick = async () => {
      if (!confirm(`Delete hook "${hook.name}"?`)) return;
      try {
        await apiFetch(`/hooks/${hookId}`, { method: 'DELETE' });
        navigate(`#/hooks/${projectId}`);
      } catch (e) {
        document.getElementById('hookDetailError').textContent = e.message;
      }
    };
  }

  async function renderArtifactDetail(projectId, artifactId) {
    let artifacts = [];
    let tools = [];
    try {
      artifacts = await apiFetch(`/projects/${projectId}/artifacts`);
      tools = await apiFetch('/tools');
    } catch (e) {
      renderShell(`<div class="card"><p class="error">${e.message}</p></div>`);
      return;
    }

    const artifact = artifacts.find(a => a.id === artifactId);
    if (!artifact) {
      renderShell(`<div class="card"><p class="error">artifact not found</p></div>`);
      return;
    }
    // Apply NDA tint (fire-and-forget)
    checkProjectNda(projectId).then(nda => applyNdaTint(nda));

    pushHistory('rc_recent_artifacts', {
      id: artifact.id,
      project_id: projectId,
      filename: artifact.filename || '',
      viewed_at: new Date().toISOString(),
    });

    const toolNames = tools.map(t => t.name);
    const quickTools = ['file.info', 'rizin.bininfo', 'strings.extract', 'vt.file_report', 'yara.scan']
      .filter(t => toolNames.includes(t));

    const options = tools
      .map(t => `<option value="${escapeHtml(t.name)}">${escapeHtml(t.name)}</option>`)
      .join('');

    renderShell(`
      <div class="card">
        <h3>Artifact ${artifact.id}</h3>
        <div class="row">
          <div><strong>Filename</strong><div class="muted">${escapeHtml(artifact.filename)}</div></div>
          <div><strong>SHA256</strong><div class="muted">${escapeHtml(artifact.sha256)}</div></div>
          <div><strong>Created</strong><div class="muted">${new Date(artifact.created_at).toLocaleString()}</div></div>
        </div>
        <div style="margin-top:10px;">
          <strong>Description</strong>
          <span id="artifactDescDisplay" class="muted" style="margin-left:8px;">${artifact.description ? escapeHtml(artifact.description) : '(none)'}</span>
          <button id="artifactDescEditBtn" class="button secondary" style="margin-left:8px;font-size:0.85em;">Edit</button>
          <span id="artifactDescEditor" style="display:none;margin-left:8px;">
            <input id="artifactDescInput" class="input" style="width:400px;" maxlength="1000" value="${artifact.description ? escapeHtml(artifact.description) : ''}" />
            <button id="artifactDescSave" class="button" style="font-size:0.85em;">Save</button>
            <button id="artifactDescCancel" class="button secondary" style="font-size:0.85em;">Cancel</button>
          </span>
        </div>
        <div class="row" style="margin-top:10px;">
          <button id="artifactDownload" class="button secondary">Download</button>
          <button id="artifactDownloadGhidra" class="button secondary">Download Ghidra Project</button>
          <button id="artifactBack" class="button secondary">Back to Project</button>
          <button id="artifactCopyId" class="button secondary">Copy Artifact ID</button>
          <button id="artifactPreview" class="button secondary">Preview Text</button>
          <button id="artifactDelete" class="button secondary" style="color:#c00;">Delete</button>
        </div>
      </div>
      <div class="card">
        <h4>Artifact Preview</h4>
        <pre id="artifactPreviewBox" class="muted">No preview loaded.</pre>
      </div>
      <div class="card">
        <h4>Run Tool on Artifact</h4>
        <div class="row" style="margin-bottom:10px;">
          ${quickTools.map(t => `<button class="button secondary quick-tool" data-tool="${escapeHtml(t)}">${escapeHtml(t)}</button>`).join('')}
        </div>
        <div class="row" style="margin-bottom:10px;">
          <input id="artifactToolName" class="input" list="artifactToolList" placeholder="tool name" />
          <datalist id="artifactToolList">${options}</datalist>
        </div>
        <div style="margin-bottom:10px;">
          <textarea id="artifactToolInput" class="input" placeholder="input JSON">{ "artifact_id": "${artifact.id}" }</textarea>
        </div>
        <button id="artifactToolRun" class="button">Run</button>
        <div id="artifactToolErr" class="error" style="margin-top:10px;"></div>
        <div class="card" style="margin-top:10px;">
          <h4>Output</h4>
          <pre id="artifactToolOutput" class="muted"></pre>
        </div>
      </div>
      <div class="card">
        <h4>Recent Tool Runs (Local)</h4>
        <div id="artifactRuns" class="muted"></div>
      </div>
      <div class="card">
        <h4>Compare Tool Runs</h4>
        <div class="row" style="margin-bottom:10px;">
          <select id="runA" class="input"></select>
          <select id="runB" class="input"></select>
          <button id="runCompare" class="button secondary">Compare</button>
        </div>
        <pre id="runDiff" class="muted">Select two runs to compare.</pre>
      </div>
    `);

    // Description inline editing
    document.getElementById('artifactDescEditBtn').onclick = () => {
      document.getElementById('artifactDescDisplay').style.display = 'none';
      document.getElementById('artifactDescEditBtn').style.display = 'none';
      document.getElementById('artifactDescEditor').style.display = 'inline';
      document.getElementById('artifactDescInput').focus();
    };
    document.getElementById('artifactDescCancel').onclick = () => {
      document.getElementById('artifactDescDisplay').style.display = '';
      document.getElementById('artifactDescEditBtn').style.display = '';
      document.getElementById('artifactDescEditor').style.display = 'none';
    };
    document.getElementById('artifactDescSave').onclick = async () => {
      const input = document.getElementById('artifactDescInput');
      const desc = input.value.trim();
      if (!desc || desc.length > 1000) { alert('Description must be 1-1000 characters.'); return; }
      try {
        const headers = { 'Content-Type': 'application/json' };
        if (state.apiKey) headers['Authorization'] = `Bearer ${state.apiKey}`;
        const res = await fetch(`${API_BASE}/artifacts/${artifact.id}`, {
          method: 'PATCH', headers, body: JSON.stringify({ description: desc }),
        });
        if (!res.ok) { const b = await res.json().catch(() => ({})); throw new Error(b.error || res.statusText); }
        const updated = await res.json();
        document.getElementById('artifactDescDisplay').textContent = updated.description || '(none)';
        artifact.description = updated.description;
      } catch (e) { alert('Failed to save: ' + e.message); }
      document.getElementById('artifactDescDisplay').style.display = '';
      document.getElementById('artifactDescEditBtn').style.display = '';
      document.getElementById('artifactDescEditor').style.display = 'none';
    };

    document.getElementById('artifactDownload').onclick = async () => {
      try {
        await downloadArtifact(artifact.id, artifact.filename);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('artifactDownloadGhidra').onclick = async () => {
      try {
        const headers = {};
        if (state.apiKey) headers['Authorization'] = `Bearer ${state.apiKey}`;
        const res = await fetch(`${API_BASE}/artifacts/${artifact.id}/ghidra-project`, { headers });
        if (!res.ok) {
          const body = await res.json().catch(() => ({}));
          throw new Error(body.error || `${res.status} ${res.statusText}`);
        }
        const blob = await res.blob();
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `ghidra-${artifact.sha256.substring(0, 8)}.zip`;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
      } catch (e) {
        alert(e.message);
      }
    };

    document.getElementById('artifactBack').onclick = () => {
      navigate(`#/projects/${projectId}`);
    };

    document.getElementById('artifactCopyId').onclick = async () => {
      await copyToClipboard(artifact.id);
      alert('Copied artifact ID.');
    };

    document.getElementById('artifactPreview').onclick = async () => {
      const box = document.getElementById('artifactPreviewBox');
      box.textContent = 'Loading preview...';
      try {
        const headers = {};
        if (state.apiKey) headers['Authorization'] = `Bearer ${state.apiKey}`;
        const res = await fetch(`${API_BASE}/artifacts/${artifact.id}/download`, { headers });
        if (!res.ok) {
          const text = await res.text();
          throw new Error(text || `${res.status} ${res.statusText}`);
        }
        const buf = await res.arrayBuffer();
        if (!isMostlyText(buf)) {
          box.textContent = 'Binary data detected. Preview not available.';
          return;
        }
        box.textContent = decodeTextPreview(buf);
      } catch (e) {
        box.textContent = `Preview failed: ${e.message}`;
      }
    };

    document.getElementById('artifactDelete').onclick = async () => {
      if (!confirm(`Delete artifact "${artifact.filename}"? This cannot be undone.`)) return;
      try {
        await apiFetch(`/artifacts/${artifact.id}`, { method: 'DELETE' });
        navigate(`#/projects/${projectId}`);
      } catch (e) {
        alert(`Delete failed: ${e.message}`);
      }
    };

    document.getElementById('artifactToolRun').onclick = async () => {
      const name = document.getElementById('artifactToolName').value.trim();
      const inputRaw = document.getElementById('artifactToolInput').value.trim() || '{}';
      if (!name) return;
      let inputJson;
      try {
        inputJson = JSON.parse(inputRaw);
      } catch (e) {
        document.getElementById('artifactToolErr').textContent = 'Input JSON is invalid.';
        return;
      }
      try {
        const result = await apiFetch(`/tools/${name}/run`, {
          method: 'POST',
          body: JSON.stringify({ project_id: projectId, input: inputJson }),
        });
        const output = {
          status: result.status,
          produced_artifacts: result.produced_artifacts || [],
          output: result.output,
        };
        document.getElementById('artifactToolOutput').textContent = JSON.stringify(output, null, 2);
        document.getElementById('artifactToolErr').textContent = '';
        const runKey = `rc_artifact_runs_${artifact.id}`;
        pushHistory(runKey, {
          id: `${name}:${Date.now()}`,
          tool: name,
          ran_at: new Date().toISOString(),
          produced_artifacts: output.produced_artifacts || [],
          output_text: JSON.stringify(output.output || {}, null, 2),
        }, 10, 'id');
        renderArtifactRuns(runKey, projectId);
        renderArtifactCompare(runKey);
      } catch (e) {
        document.getElementById('artifactToolErr').textContent = e.message;
      }
    };

    document.querySelectorAll('.quick-tool').forEach(btn => {
      btn.onclick = () => {
        const name = btn.dataset.tool;
        document.getElementById('artifactToolName').value = name;
        document.getElementById('artifactToolInput').value = JSON.stringify(
          { artifact_id: artifact.id },
          null,
          2
        );
      };
    });
    renderArtifactRuns(`rc_artifact_runs_${artifact.id}`, projectId);
    renderArtifactCompare(`rc_artifact_runs_${artifact.id}`);
  }

  function renderArtifactRuns(key, projectId) {
    const host = document.getElementById('artifactRuns');
    if (!host) return;
    const runs = readLocalArray(key);
    if (!runs.length) {
      host.textContent = 'No local runs yet.';
      return;
    }
    host.innerHTML = runs.map(r => {
      const produced = (r.produced_artifacts || []).map(id => `
        <a href="#/projects/${projectId}/artifacts/${id}">${id}</a>
      `).join(' ');
      return `
        <div class="card">
          <div class="muted">${escapeHtml(r.tool || '')} · ${new Date(r.ran_at).toLocaleString()}</div>
          <div>Produced: ${produced || '<span class="muted">none</span>'}</div>
        </div>
      `;
    }).join('');
  }

  function renderArtifactCompare(key) {
    const runs = readLocalArray(key);
    const selectA = document.getElementById('runA');
    const selectB = document.getElementById('runB');
    const diffBox = document.getElementById('runDiff');
    if (!selectA || !selectB || !diffBox) return;
    if (!runs.length) {
      selectA.innerHTML = '';
      selectB.innerHTML = '';
      diffBox.textContent = 'No runs to compare.';
      return;
    }
    const options = runs.map(r => `<option value="${escapeHtml(r.id)}">${escapeHtml(r.tool || '')} · ${new Date(r.ran_at).toLocaleString()}</option>`).join('');
    selectA.innerHTML = options;
    selectB.innerHTML = options;
    document.getElementById('runCompare').onclick = () => {
      const a = runs.find(r => r.id === selectA.value);
      const b = runs.find(r => r.id === selectB.value);
      if (!a || !b) {
        diffBox.textContent = 'Select two runs.';
        return;
      }
      diffBox.textContent = diffLines(a.output_text || '', b.output_text || '');
    };
  }

  async function renderAudit() {
    renderShell(`
      <div class="card">
        <h3>Audit Log (Admin)</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="auditType" class="input" placeholder="event type (optional)" />
          <input id="auditLimit" class="input" placeholder="limit (default 50)" />
          <button id="auditLoad" class="button">Load</button>
        </div>
        <div id="auditErr" class="error"></div>
        <table class="table" id="auditTable">
          <thead><tr><th>Time</th><th>Type</th><th>Actor</th><th>Detail</th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
    `);

    const load = async () => {
      const type = document.getElementById('auditType').value.trim();
      const limit = document.getElementById('auditLimit').value.trim();
      const params = new URLSearchParams();
      if (type) params.set('type', type);
      if (limit) params.set('limit', limit);
      const qs = params.toString();
      try {
        const rows = await apiFetch(`/audit${qs ? `?${qs}` : ''}`);
        const body = rows.map(r => `
          <tr>
            <td>${new Date(r.created_at).toLocaleString()}</td>
            <td>${escapeHtml(r.event_type)}</td>
            <td>${escapeHtml(r.actor_subject || r.actor_user_id || '')}</td>
            <td><pre>${escapeHtml(JSON.stringify(r.detail || {}, null, 2))}</pre></td>
          </tr>
        `).join('');
        document.querySelector('#auditTable tbody').innerHTML = body || '';
        document.getElementById('auditErr').textContent = '';
      } catch (e) {
        document.getElementById('auditErr').textContent = e.message;
      }
    };

    document.getElementById('auditLoad').onclick = load;
    load();
  }

  async function renderSearch() {
    renderShell(`
      <div class="card">
        <h3>Local Search</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="searchQuery" class="input" placeholder="Search local history" />
          <button id="searchRun" class="button">Search</button>
          <button id="searchSave" class="button secondary">Save</button>
        </div>
        <div id="searchSaved" class="muted"></div>
        <div id="searchResults" class="muted"></div>
      </div>
    `);

    const runSearch = () => {
      const q = document.getElementById('searchQuery').value.trim().toLowerCase();
      if (!q) {
        document.getElementById('searchResults').textContent = 'Enter a search term.';
        return;
      }
      const results = [];
      const threads = readLocalArray('rc_recent_threads');
      for (const t of threads) {
        const hay = `${t.id} ${t.title || ''} ${t.agent || ''}`.toLowerCase();
        if (hay.includes(q)) {
          results.push(`<div><strong>Conversation</strong>: <a href="#/thread/${t.id}">${t.id}</a> ${escapeHtml(t.title || '')}</div>`);
        }
      }
      const artifacts = readLocalArray('rc_recent_artifacts');
      for (const a of artifacts) {
        const hay = `${a.id} ${a.filename || ''} ${a.project_id || ''}`.toLowerCase();
        if (hay.includes(q)) {
          results.push(`<div><strong>Artifact</strong>: <a href="#/projects/${a.project_id}/artifacts/${a.id}">${a.id}</a> ${escapeHtml(a.filename || '')}</div>`);
        }
      }
      const runKeys = Object.keys(localStorage).filter(k => k.startsWith('rc_workflow_runs_') || k.startsWith('rc_artifact_runs_'));
      for (const key of runKeys) {
        const items = readLocalArray(key);
        for (const item of items) {
          const hay = JSON.stringify(item).toLowerCase();
          if (hay.includes(q)) {
            if (key.startsWith('rc_workflow_runs_')) {
              results.push(`<div><strong>Workflow Run</strong>: ${escapeHtml(item.workflow || '')} · ${escapeHtml(item.route || '')}</div>`);
            } else {
              results.push(`<div><strong>Artifact Run</strong>: ${escapeHtml(item.tool || '')}</div>`);
            }
          }
        }
      }
      document.getElementById('searchResults').innerHTML = results.length ? results.join('') : 'No matches.';
    };

    document.getElementById('searchRun').onclick = runSearch;
    document.getElementById('searchSave').onclick = () => {
      const q = document.getElementById('searchQuery').value.trim();
      if (!q) return;
      pushHistory('rc_saved_queries', { id: q, query: q }, 20, 'id');
      renderSavedQueries();
    };
    document.getElementById('searchQuery').onkeypress = (e) => {
      if (e.key === 'Enter') runSearch();
    };

    const renderSavedQueries = () => {
      const saved = readLocalArray('rc_saved_queries');
      const host = document.getElementById('searchSaved');
      if (!saved.length) {
        host.textContent = 'No saved queries.';
        return;
      }
      host.innerHTML = saved.map(s => `
        <div class="row" style="margin-bottom:6px;">
          <div>${escapeHtml(s.query)}</div>
          <button class="button secondary saved-run" data-q="${escapeHtml(s.query)}">Run</button>
          <button class="button secondary saved-del" data-q="${escapeHtml(s.query)}">Delete</button>
        </div>
      `).join('');
      host.querySelectorAll('.saved-run').forEach(btn => {
        btn.onclick = () => {
          document.getElementById('searchQuery').value = btn.dataset.q;
          runSearch();
        };
      });
      host.querySelectorAll('.saved-del').forEach(btn => {
        btn.onclick = () => {
          const q = btn.dataset.q;
          const updated = readLocalArray('rc_saved_queries').filter(s => s.query !== q);
          writeLocalArray('rc_saved_queries', updated);
          renderSavedQueries();
        };
      });
    };

    renderSavedQueries();
    document.getElementById('searchQuery').focus();
  }

  async function renderAdmin() {
    renderShell(`
      <div class="card">
        <h3>My Quota</h3>
        <div id="quotaSelf" class="muted">Loading...</div>
      </div>
      <div class="card">
        <h3>Admin: User Quota</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="quotaUserId" class="input" placeholder="User UUID" />
          <button id="quotaLoad" class="button">Load</button>
        </div>
        <div id="quotaAdmin" class="muted"></div>
        <div id="quotaErr" class="error"></div>
      </div>
      <div class="card">
        <h3>Admin: Users</h3>
        <div class="row" style="margin-bottom:10px;">
          <button id="usersLoad" class="button secondary">Refresh Users</button>
        </div>
        <div id="usersErr" class="error"></div>
        <table class="table" id="usersTable">
          <thead><tr><th>Subject</th><th>Roles</th><th>Enabled</th><th>ID</th><th>Updated</th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="card">
        <h4>Create User</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="userSubject" class="input" placeholder="subject (required)" />
          <input id="userDisplay" class="input" placeholder="display name (optional)" />
        </div>
        <div class="row" style="margin-bottom:10px;">
          <input id="userEmail" class="input" placeholder="email (optional)" />
          <input id="userRoles" class="input" placeholder="roles (comma or newline, default operator)" />
        </div>
        <button id="userCreate" class="button">Create User</button>
      </div>
      <div class="card">
        <h3>Admin: API Keys</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="apiKeyUserId" class="input" placeholder="User UUID" />
          <button id="apiKeysLoad" class="button">Load Keys</button>
        </div>
        <div class="row" style="margin-bottom:10px;">
          <input id="apiKeyFilter" class="input" placeholder="Filter by name or prefix" />
        </div>
        <div id="apiKeyErr" class="error"></div>
        <table class="table" id="apiKeysTable">
          <thead><tr><th>Name</th><th>Prefix</th><th>ID</th><th>Created</th><th>Last Used</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div class="row" style="margin-top:10px;">
          <input id="apiKeyName" class="input" placeholder="New key name" />
          <button id="apiKeyCreate" class="button">Create Key</button>
        </div>
        <div id="apiKeyCreateResult" class="card" style="margin-top:10px; display:none;"></div>
      </div>
      <div class="card">
        <h3>Admin: Model Access</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="routesUserId" class="input" placeholder="User UUID" />
          <button id="routesLoad" class="button">Load Routes</button>
        </div>
        <div id="routesErr" class="error"></div>
        <div id="routesList" class="muted"></div>
        <div class="row" style="margin-top:10px;">
          <input id="routeAdd" class="input" list="routeDatalist" placeholder="Route to add (e.g. openai:gpt-4o)" />
          <datalist id="routeDatalist"></datalist>
          <button id="routeAddBtn" class="button">Add</button>
          <button id="routesClear" class="button secondary">Clear All</button>
        </div>
      </div>
      <div class="card">
        <h3>Admin: Tool Grants</h3>
        <h4>Restricted Tools</h4>
        <div id="restrictedErr" class="error"></div>
        <table class="table" id="restrictedTable">
          <thead><tr><th>Pattern</th><th>Description</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div class="row" style="margin-top:8px;">
          <input id="restrictPattern" class="input" placeholder="Tool pattern (e.g. web.*)" />
          <input id="restrictDesc" class="input" placeholder="Description" />
          <button id="restrictAdd" class="button">Restrict</button>
        </div>
        <h4 style="margin-top:16px;">User Tool Grants</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="grantUserId" class="input" placeholder="User UUID" />
          <button id="grantsLoad" class="button">Load Grants</button>
        </div>
        <div id="grantsErr" class="error"></div>
        <table class="table" id="grantsTable">
          <thead><tr><th>Pattern</th><th>Granted</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div class="row" style="margin-top:8px;">
          <input id="grantPattern" class="input" placeholder="Tool pattern to grant" />
          <button id="grantAdd" class="button">Grant</button>
        </div>
      </div>
      <div class="card">
        <h3>Admin: Web URL Rules</h3>
        <div id="webRulesErr" class="error"></div>
        <table class="table" id="webRulesTable">
          <thead><tr><th>ID</th><th>Type</th><th>Scope</th><th>Pattern Type</th><th>Pattern</th><th>Description</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <h4 style="margin-top:12px;">Add Rule</h4>
        <div class="row" style="margin-bottom:8px;">
          <select id="wrScope" class="input"><option value="global">Global</option><option value="project">Project</option></select>
          <input id="wrProjectId" class="input" placeholder="Project UUID (if project scope)" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <label><input type="radio" name="wrType" value="block" checked /> Block</label>
          <label style="margin-left:12px;"><input type="radio" name="wrType" value="allow" /> Allow</label>
        </div>
        <div class="row" style="margin-bottom:8px;">
          <select id="wrPatternType" class="input">
            <option value="domain">domain</option>
            <option value="domain_suffix">domain_suffix</option>
            <option value="url_prefix">url_prefix</option>
            <option value="url_regex">url_regex</option>
            <option value="ip_cidr">ip_cidr</option>
          </select>
          <input id="wrPattern" class="input" placeholder="Pattern value" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <input id="wrDesc" class="input" placeholder="Description (optional)" />
          <button id="wrAdd" class="button">Add Rule</button>
        </div>
      </div>
      <div class="card">
        <h3>Admin: Email Recipient Rules</h3>
        <div id="emailRulesErr" class="error"></div>
        <table class="table" id="emailRulesTable">
          <thead><tr><th>ID</th><th>Type</th><th>Target Type</th><th>Pattern</th><th>Scope</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <h4 style="margin-top:12px;">Add Rule</h4>
        <div class="row" style="margin-bottom:8px;">
          <label><input type="radio" name="erType" value="block" checked /> Block</label>
          <label style="margin-left:12px;"><input type="radio" name="erType" value="allow" /> Allow</label>
        </div>
        <div class="row" style="margin-bottom:8px;">
          <select id="erPatternType" class="input">
            <option value="email">email</option>
            <option value="domain">domain</option>
            <option value="domain_suffix">domain_suffix</option>
          </select>
          <input id="erPattern" class="input" placeholder="Target (e.g. user@example.com)" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <input id="erDesc" class="input" placeholder="Description (optional)" />
          <button id="erAdd" class="button">Add Rule</button>
        </div>
      </div>
      <div class="card">
        <h3>Admin: Email Accounts &amp; Scheduled</h3>
        <h4>Credentials</h4>
        <div id="emailCredsErr" class="error"></div>
        <table class="table" id="emailCredsTable">
          <thead><tr><th>ID</th><th>Provider</th><th>Email</th><th>Default</th><th>Created</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <h4 style="margin-top:16px;">Tone Presets</h4>
        <div id="emailTonesErr" class="error"></div>
        <table class="table" id="emailTonesTable">
          <thead><tr><th>Name</th><th>Description</th><th>Builtin</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div class="row" style="margin-top:8px;">
          <input id="toneName" class="input" placeholder="Name" />
          <input id="toneDesc" class="input" placeholder="Description" />
          <input id="toneInstr" class="input" placeholder="System instruction" />
          <button id="toneAdd" class="button">Add Tone</button>
        </div>
        <h4 style="margin-top:16px;">Scheduled Emails</h4>
        <div id="emailSchedErr" class="error"></div>
        <table class="table" id="emailSchedTable">
          <thead><tr><th>ID</th><th>To</th><th>Subject</th><th>Scheduled</th><th>Status</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="card">
        <h3>Admin: YARA Rules</h3>
        <div id="yaraErr" class="error"></div>
        <table class="table" id="yaraRulesTable">
          <thead><tr><th>ID</th><th>Name</th><th>Description</th><th>Tags</th><th>Scope</th><th>Created</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div id="yaraRuleDetail" style="display:none; margin-top:12px;">
          <h4>Rule Source</h4>
          <pre id="yaraRuleSource" style="max-height:300px; overflow:auto; background:var(--bg-secondary); padding:8px; border-radius:4px;"></pre>
        </div>
        <h4 style="margin-top:16px;">Scan Results</h4>
        <div class="row" style="margin-bottom:10px;">
          <input id="yaraScanArtifact" class="input" placeholder="Artifact UUID" />
          <button id="yaraScanLoad" class="button">Load Results</button>
        </div>
        <div id="yaraScanErr" class="error"></div>
        <table class="table" id="yaraScanTable">
          <thead><tr><th>Rule</th><th>Matches</th><th>Matched At</th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="card">
        <h3>Available Models</h3>
        <div id="modelsOverview" class="muted">Loading...</div>
      </div>
    `);

    const renderQuota = (data) => {
      const q = data.quota;
      const u = data.usage;
      const ar = data.allowed_routes || [];
      const routesHtml = ar.length === 0
        ? '<span class="muted">All models (unrestricted)</span>'
        : ar.map(r => `<span class="badge">${escapeHtml(r)}</span>`).join(' ');
      return `
        <div class="card">
          <div class="row">
            <div><strong>Storage</strong><div class="muted">${q.max_storage_bytes}</div></div>
            <div><strong>Projects</strong><div class="muted">${q.max_projects}</div></div>
            <div><strong>Concurrent Runs</strong><div class="muted">${q.max_concurrent_runs}</div></div>
          </div>
          <div class="row" style="margin-top:8px;">
            <div><strong>LLM Tokens/Day</strong><div class="muted">${q.max_llm_tokens_per_day}</div></div>
            <div><strong>Upload Bytes</strong><div class="muted">${q.max_upload_bytes}</div></div>
            <div><strong>VT Lookups/Day</strong><div class="muted">${q.max_vt_lookups_per_day}</div></div>
          </div>
          <div class="row" style="margin-top:8px;">
            <div><strong>Usage: Prompt</strong><div class="muted">${u.llm_prompt_tokens}</div></div>
            <div><strong>Usage: Completion</strong><div class="muted">${u.llm_completion_tokens}</div></div>
            <div><strong>Usage: VT</strong><div class="muted">${u.vt_lookups}</div></div>
            <div><strong>Usage: Tool Runs</strong><div class="muted">${u.tool_runs}</div></div>
          </div>
          <div style="margin-top:8px;">
            <strong>Allowed Models</strong>
            <div style="margin-top:4px;">${routesHtml}</div>
          </div>
        </div>
      `;
    };

    try {
      const me = await apiFetch('/quota');
      document.getElementById('quotaSelf').innerHTML = renderQuota(me);
    } catch (e) {
      document.getElementById('quotaSelf').innerHTML = `<span class="error">${escapeHtml(e.message)}</span>`;
    }

    document.getElementById('quotaLoad').onclick = async () => {
      const userId = document.getElementById('quotaUserId').value.trim();
      if (!userId) return;
      try {
        const data = await apiFetch(`/admin/quota/${userId}`);
        document.getElementById('quotaAdmin').innerHTML = renderQuota(data) + `
          <div class="card">
            <h4>Update Quota</h4>
            <div class="row"><input id="qStorage" class="input" placeholder="max_storage_bytes" /></div>
            <div class="row"><input id="qProjects" class="input" placeholder="max_projects" /></div>
            <div class="row"><input id="qRuns" class="input" placeholder="max_concurrent_runs" /></div>
            <div class="row"><input id="qTokens" class="input" placeholder="max_llm_tokens_per_day" /></div>
            <div class="row"><input id="qUpload" class="input" placeholder="max_upload_bytes" /></div>
            <div class="row"><input id="qVT" class="input" placeholder="max_vt_lookups_per_day" /></div>
            <div class="row" style="margin-top:8px;">
              <button id="qUpdate" class="button">Update</button>
            </div>
          </div>
        `;
        document.getElementById('quotaErr').textContent = '';

        document.getElementById('qUpdate').onclick = async () => {
          const payload = {};
          const map = [
            ['qStorage', 'max_storage_bytes'],
            ['qProjects', 'max_projects'],
            ['qRuns', 'max_concurrent_runs'],
            ['qTokens', 'max_llm_tokens_per_day'],
            ['qUpload', 'max_upload_bytes'],
            ['qVT', 'max_vt_lookups_per_day'],
          ];
          for (const [id, key] of map) {
            const v = document.getElementById(id).value.trim();
            if (v) payload[key] = Number(v);
          }
          if (Object.keys(payload).length === 0) return;
          try {
            await apiFetch(`/admin/quota/${userId}`, {
              method: 'PUT',
              body: JSON.stringify(payload),
            });
            alert('Quota updated.');
          } catch (e) {
            alert(e.message);
          }
        };
      } catch (e) {
        document.getElementById('quotaErr').textContent = e.message;
      }
    };

    const renderUsers = (rows) => rows.map(u => `
      <tr>
        <td><button class="button secondary user-select" data-id="${u.id}">${escapeHtml(u.subject || u.id)}</button></td>
        <td>${escapeHtml((u.roles || []).join(','))}</td>
        <td>${u.enabled ? 'yes' : 'no'}</td>
        <td class="muted">${u.id}</td>
        <td>${new Date(u.updated_at).toLocaleString()}</td>
      </tr>
    `).join('');

    const loadUsers = async () => {
      try {
        const rows = await apiFetch('/admin/users');
        document.querySelector('#usersTable tbody').innerHTML = renderUsers(rows);
        document.getElementById('usersErr').textContent = '';
        document.querySelectorAll('.user-select').forEach(btn => {
          btn.onclick = () => {
            const id = btn.dataset.id;
            document.getElementById('quotaUserId').value = id;
            document.getElementById('apiKeyUserId').value = id;
            document.getElementById('routesUserId').value = id;
          };
        });
      } catch (e) {
        document.getElementById('usersErr').textContent = e.message;
      }
    };

    document.getElementById('usersLoad').onclick = loadUsers;
    loadUsers();

    document.getElementById('userCreate').onclick = async () => {
      const subject = document.getElementById('userSubject').value.trim();
      const displayName = document.getElementById('userDisplay').value.trim();
      const email = document.getElementById('userEmail').value.trim();
      const rolesRaw = document.getElementById('userRoles').value.trim();
      if (!subject) return;
      const roles = rolesRaw ? parseRoles(rolesRaw) : undefined;
      const payload = {
        subject,
        display_name: displayName || null,
        email: email || null,
        roles: roles && roles.length ? roles : undefined,
      };
      try {
        await apiFetch('/admin/users', {
          method: 'POST',
          body: JSON.stringify(payload),
        });
        document.getElementById('userSubject').value = '';
        document.getElementById('userDisplay').value = '';
        document.getElementById('userEmail').value = '';
        document.getElementById('userRoles').value = '';
        await loadUsers();
      } catch (e) {
        document.getElementById('usersErr').textContent = e.message;
      }
    };

    let apiKeyCache = [];
    const renderApiKeys = (rows) => rows.map(k => `
      <tr class="${k.last_used_at && (Date.now() - new Date(k.last_used_at).getTime()) < 86400000 ? 'row-highlight' : ''}">
        <td>${escapeHtml(k.name || '')}</td>
        <td>${escapeHtml(k.key_prefix || '')} <button class="button secondary api-key-copy" data-prefix="${escapeHtml(k.key_prefix || '')}">Copy</button></td>
        <td class="muted">${k.id}</td>
        <td>${new Date(k.created_at).toLocaleString()}</td>
        <td>${k.last_used_at ? new Date(k.last_used_at).toLocaleString() : ''}</td>
        <td><button class="button secondary api-key-revoke" data-id="${k.id}">Revoke</button></td>
      </tr>
    `).join('');

    const applyApiKeyFilter = () => {
      const q = document.getElementById('apiKeyFilter').value.trim().toLowerCase();
      const filtered = q
        ? apiKeyCache.filter(k => {
            const hay = `${k.name || ''} ${k.key_prefix || ''}`.toLowerCase();
            return hay.includes(q);
          })
        : apiKeyCache;
      document.querySelector('#apiKeysTable tbody').innerHTML = renderApiKeys(filtered);
      document.querySelectorAll('.api-key-revoke').forEach(btn => {
        btn.onclick = async () => {
          if (!confirm('Revoke this API key?')) return;
          try {
            await apiFetch(`/admin/api_keys/${btn.dataset.id}`, { method: 'DELETE' });
            await loadKeys();
          } catch (e) {
            document.getElementById('apiKeyErr').textContent = e.message;
          }
        };
      });
      document.querySelectorAll('.api-key-copy').forEach(btn => {
        btn.onclick = async () => {
          await copyToClipboard(btn.dataset.prefix || '');
          alert('Copied key prefix.');
        };
      });
    };

    const loadKeys = async () => {
      const userId = document.getElementById('apiKeyUserId').value.trim();
      if (!userId) return;
      try {
        apiKeyCache = await apiFetch(`/admin/users/${userId}/api_keys`);
        applyApiKeyFilter();
        document.getElementById('apiKeyErr').textContent = '';
      } catch (e) {
        document.getElementById('apiKeyErr').textContent = e.message;
      }
    };

    document.getElementById('apiKeysLoad').onclick = loadKeys;
    document.getElementById('apiKeyFilter').oninput = applyApiKeyFilter;

    document.getElementById('apiKeyCreate').onclick = async () => {
      const userId = document.getElementById('apiKeyUserId').value.trim();
      const name = document.getElementById('apiKeyName').value.trim();
      if (!userId || !name) return;
      try {
        const result = await apiFetch(`/admin/users/${userId}/api_keys`, {
          method: 'POST',
          body: JSON.stringify({ name }),
        });
        const card = document.getElementById('apiKeyCreateResult');
        card.style.display = 'block';
        card.innerHTML = `
          <h4>New API Key</h4>
          <div class="muted">Copy this now. You won't see it again.</div>
          <pre>${escapeHtml(result.raw_key)}</pre>
          <button id="copyNewKey" class="button secondary">Copy Key</button>
        `;
        document.getElementById('copyNewKey').onclick = async () => {
          await copyToClipboard(result.raw_key);
          alert('Copied API key.');
        };
        document.getElementById('apiKeyName').value = '';
        await loadKeys();
      } catch (e) {
        document.getElementById('apiKeyErr').textContent = e.message;
      }
    };

    // --- Model Access (routes) ---
    const renderRoutesList = (routes, unrestricted) => {
      if (unrestricted) {
        return '<div class="muted" style="margin-top:8px;">Unrestricted (all models allowed)</div>';
      }
      return '<div style="margin-top:8px;">' + routes.map(r =>
        `<span class="badge" style="margin:2px;">${escapeHtml(r)} <button class="route-remove" data-route="${escapeHtml(r)}" style="border:none;background:none;cursor:pointer;font-size:0.9em;padding:0 2px;">x</button></span>`
      ).join(' ') + '</div>';
    };

    const loadRoutes = async () => {
      const userId = document.getElementById('routesUserId').value.trim();
      if (!userId) return;
      try {
        const data = await apiFetch(`/admin/users/${userId}/routes`);
        document.getElementById('routesList').innerHTML = renderRoutesList(data.routes, data.unrestricted);
        document.getElementById('routesErr').textContent = '';
        document.querySelectorAll('.route-remove').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/users/${userId}/routes`, {
                method: 'DELETE',
                body: JSON.stringify({ route: btn.dataset.route }),
              });
              await loadRoutes();
            } catch (e) {
              document.getElementById('routesErr').textContent = e.message;
            }
          };
        });
      } catch (e) {
        document.getElementById('routesErr').textContent = e.message;
      }
    };

    document.getElementById('routesLoad').onclick = loadRoutes;

    document.getElementById('routeAddBtn').onclick = async () => {
      const userId = document.getElementById('routesUserId').value.trim();
      const route = document.getElementById('routeAdd').value.trim();
      if (!userId || !route) return;
      try {
        await apiFetch(`/admin/users/${userId}/routes`, {
          method: 'POST',
          body: JSON.stringify({ route }),
        });
        document.getElementById('routeAdd').value = '';
        await loadRoutes();
      } catch (e) {
        document.getElementById('routesErr').textContent = e.message;
      }
    };

    document.getElementById('routesClear').onclick = async () => {
      const userId = document.getElementById('routesUserId').value.trim();
      if (!userId) return;
      if (!confirm('Remove all route restrictions for this user?')) return;
      try {
        await apiFetch(`/admin/users/${userId}/routes`, {
          method: 'DELETE',
          body: JSON.stringify({ clear: true }),
        });
        await loadRoutes();
      } catch (e) {
        document.getElementById('routesErr').textContent = e.message;
      }
    };

    // --- Models Overview ---
    try {
      const bd = backendsCache || await apiFetch('/llm/backends').catch(() => null);
      if (bd && bd.backends) {
        backendsCache = bd;
        const routeOpts = (bd.routes || []).concat(bd.backends.map(b => b.name));
        const dl = document.getElementById('routeDatalist');
        if (dl) dl.innerHTML = [...new Set(routeOpts)].map(r => `<option value="${escapeHtml(r)}">`).join('');

        const rows = bd.backends.map(b => `
          <tr>
            <td>${escapeHtml(b.name)}</td>
            <td>${b.context_window ? formatCtx(b.context_window) : ''}</td>
            <td>${b.max_output_tokens ? formatCtx(b.max_output_tokens) : ''}</td>
            <td>${b.cost_per_mtok_input != null ? '$' + b.cost_per_mtok_input + ' / $' + b.cost_per_mtok_output : ''}</td>
            <td>${b.supports_vision ? 'yes' : ''}</td>
            <td>${b.supports_streaming ? 'yes' : ''}</td>
            <td>${b.supports_tool_calls ? 'yes' : ''}</td>
            <td>${b.knowledge_cutoff || ''}</td>
          </tr>
        `).join('');
        document.getElementById('modelsOverview').innerHTML = `
          <table class="table">
            <thead><tr><th>Model</th><th>Context</th><th>Max Out</th><th>Cost (In/Out)</th><th>Vision</th><th>Stream</th><th>Tools</th><th>Cutoff</th></tr></thead>
            <tbody>${rows}</tbody>
          </table>
        `;
      } else {
        document.getElementById('modelsOverview').innerHTML = '<span class="muted">No backends configured</span>';
      }
    } catch (_) {}

    // --- Tool Grants ---
    const loadRestricted = async () => {
      try {
        const rows = await apiFetch('/admin/restricted-tools');
        document.querySelector('#restrictedTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td>${escapeHtml(r.tool_pattern)}</td>
            <td>${escapeHtml(r.description || '')}</td>
            <td><button class="button secondary restricted-del" data-pattern="${escapeHtml(r.tool_pattern)}">Remove</button></td>
          </tr>
        `).join('');
        document.getElementById('restrictedErr').textContent = '';
        document.querySelectorAll('.restricted-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch('/admin/restricted-tools', {
                method: 'DELETE',
                body: JSON.stringify({ tool_pattern: btn.dataset.pattern }),
              });
              await loadRestricted();
            } catch (e) { document.getElementById('restrictedErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('restrictedErr').textContent = e.message; }
    };
    loadRestricted();

    document.getElementById('restrictAdd').onclick = async () => {
      const pattern = document.getElementById('restrictPattern').value.trim();
      const desc = document.getElementById('restrictDesc').value.trim();
      if (!pattern) return;
      try {
        await apiFetch('/admin/restricted-tools', {
          method: 'POST',
          body: JSON.stringify({ tool_pattern: pattern, description: desc || pattern }),
        });
        document.getElementById('restrictPattern').value = '';
        document.getElementById('restrictDesc').value = '';
        await loadRestricted();
      } catch (e) { document.getElementById('restrictedErr').textContent = e.message; }
    };

    const loadGrants = async () => {
      const userId = document.getElementById('grantUserId').value.trim();
      if (!userId) return;
      try {
        const rows = await apiFetch(`/admin/users/${userId}/tool-grants`);
        document.querySelector('#grantsTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td>${escapeHtml(r.tool_pattern)}</td>
            <td>${new Date(r.granted_at || r.created_at).toLocaleString()}</td>
            <td><button class="button secondary grant-del" data-pattern="${escapeHtml(r.tool_pattern)}">Revoke</button></td>
          </tr>
        `).join('');
        document.getElementById('grantsErr').textContent = '';
        document.querySelectorAll('.grant-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/users/${userId}/tool-grants`, {
                method: 'DELETE',
                body: JSON.stringify({ tool_pattern: btn.dataset.pattern }),
              });
              await loadGrants();
            } catch (e) { document.getElementById('grantsErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('grantsErr').textContent = e.message; }
    };
    document.getElementById('grantsLoad').onclick = loadGrants;

    document.getElementById('grantAdd').onclick = async () => {
      const userId = document.getElementById('grantUserId').value.trim();
      const pattern = document.getElementById('grantPattern').value.trim();
      if (!userId || !pattern) return;
      try {
        await apiFetch(`/admin/users/${userId}/tool-grants`, {
          method: 'POST',
          body: JSON.stringify({ tool_pattern: pattern }),
        });
        document.getElementById('grantPattern').value = '';
        await loadGrants();
      } catch (e) { document.getElementById('grantsErr').textContent = e.message; }
    };

    // --- Web URL Rules ---
    const loadWebRules = async () => {
      try {
        const rows = await apiFetch('/web-rules');
        document.querySelector('#webRulesTable tbody').innerHTML = rows.map(r => {
          const scope = r.project_id ? `project:${r.project_id.substring(0,8)}` : 'global';
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td>${escapeHtml(r.rule_type)}</td>
            <td>${escapeHtml(scope)}</td>
            <td>${escapeHtml(r.pattern_type)}</td>
            <td>${escapeHtml(r.pattern)}</td>
            <td>${escapeHtml(r.description || '')}</td>
            <td><button class="button secondary wr-del" data-id="${r.id}">Delete</button></td>
          </tr>`;
        }).join('');
        document.getElementById('webRulesErr').textContent = '';
        document.querySelectorAll('.wr-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/web-rules/${btn.dataset.id}`, { method: 'DELETE' });
              await loadWebRules();
            } catch (e) { document.getElementById('webRulesErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('webRulesErr').textContent = e.message; }
    };
    loadWebRules();

    document.getElementById('wrAdd').onclick = async () => {
      const scope = document.getElementById('wrScope').value;
      const projectId = document.getElementById('wrProjectId').value.trim() || null;
      const ruleType = document.querySelector('input[name="wrType"]:checked').value;
      const patternType = document.getElementById('wrPatternType').value;
      const pattern = document.getElementById('wrPattern').value.trim();
      const desc = document.getElementById('wrDesc').value.trim() || null;
      if (!pattern) return;
      try {
        await apiFetch('/web-rules', {
          method: 'POST',
          body: JSON.stringify({ scope, project_id: projectId, rule_type: ruleType, pattern_type: patternType, pattern, description: desc }),
        });
        document.getElementById('wrPattern').value = '';
        document.getElementById('wrDesc').value = '';
        await loadWebRules();
      } catch (e) { document.getElementById('webRulesErr').textContent = e.message; }
    };

    // --- Email Recipient Rules ---
    const loadEmailRules = async () => {
      try {
        const rows = await apiFetch('/admin/email/rules');
        document.querySelector('#emailRulesTable tbody').innerHTML = rows.map(r => {
          const scope = r.project_id ? `project:${r.project_id.substring(0,8)}` : 'global';
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td>${escapeHtml(r.rule_type)}</td>
            <td>${escapeHtml(r.pattern_type)}</td>
            <td>${escapeHtml(r.pattern)}</td>
            <td>${escapeHtml(scope)}</td>
            <td><button class="button secondary er-del" data-id="${r.id}">Delete</button></td>
          </tr>`;
        }).join('');
        document.getElementById('emailRulesErr').textContent = '';
        document.querySelectorAll('.er-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/email/rules/${btn.dataset.id}`, { method: 'DELETE' });
              await loadEmailRules();
            } catch (e) { document.getElementById('emailRulesErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailRulesErr').textContent = e.message; }
    };
    loadEmailRules();

    document.getElementById('erAdd').onclick = async () => {
      const ruleType = document.querySelector('input[name="erType"]:checked').value;
      const patternType = document.getElementById('erPatternType').value;
      const pattern = document.getElementById('erPattern').value.trim();
      const desc = document.getElementById('erDesc').value.trim() || null;
      if (!pattern) return;
      try {
        await apiFetch('/admin/email/rules', {
          method: 'POST',
          body: JSON.stringify({ scope: 'global', project_id: null, rule_type: ruleType, pattern_type: patternType, pattern, description: desc }),
        });
        document.getElementById('erPattern').value = '';
        document.getElementById('erDesc').value = '';
        await loadEmailRules();
      } catch (e) { document.getElementById('emailRulesErr').textContent = e.message; }
    };

    // --- Email Accounts & Scheduled ---
    const loadEmailCreds = async () => {
      try {
        const rows = await apiFetch('/admin/email/credentials');
        document.querySelector('#emailCredsTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td>${escapeHtml(r.provider)}</td>
            <td>${escapeHtml(r.email_address)}</td>
            <td>${r.is_default ? 'yes' : ''}</td>
            <td>${new Date(r.created_at).toLocaleString()}</td>
            <td><button class="button secondary cred-del" data-id="${r.id}">Delete</button></td>
          </tr>
        `).join('');
        document.getElementById('emailCredsErr').textContent = '';
        document.querySelectorAll('.cred-del').forEach(btn => {
          btn.onclick = async () => {
            if (!confirm('Remove this email credential?')) return;
            try {
              await apiFetch(`/admin/email/credentials/${btn.dataset.id}`, { method: 'DELETE' });
              await loadEmailCreds();
            } catch (e) { document.getElementById('emailCredsErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailCredsErr').textContent = e.message; }
    };
    loadEmailCreds();

    const loadTones = async () => {
      try {
        const rows = await apiFetch('/admin/email/tones');
        document.querySelector('#emailTonesTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td>${escapeHtml(r.name)}</td>
            <td>${escapeHtml(r.description || '')}</td>
            <td>${r.is_builtin ? 'yes' : ''}</td>
            <td>${r.is_builtin ? '' : `<button class="button secondary tone-del" data-name="${escapeHtml(r.name)}">Delete</button>`}</td>
          </tr>
        `).join('');
        document.getElementById('emailTonesErr').textContent = '';
        document.querySelectorAll('.tone-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/email/tones/${encodeURIComponent(btn.dataset.name)}`, { method: 'DELETE' });
              await loadTones();
            } catch (e) { document.getElementById('emailTonesErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailTonesErr').textContent = e.message; }
    };
    loadTones();

    document.getElementById('toneAdd').onclick = async () => {
      const name = document.getElementById('toneName').value.trim();
      const desc = document.getElementById('toneDesc').value.trim();
      const instr = document.getElementById('toneInstr').value.trim();
      if (!name || !instr) return;
      try {
        await apiFetch('/admin/email/tones', {
          method: 'POST',
          body: JSON.stringify({ name, description: desc || null, system_instruction: instr }),
        });
        document.getElementById('toneName').value = '';
        document.getElementById('toneDesc').value = '';
        document.getElementById('toneInstr').value = '';
        await loadTones();
      } catch (e) { document.getElementById('emailTonesErr').textContent = e.message; }
    };

    const loadScheduled = async () => {
      try {
        const rows = await apiFetch('/admin/email/scheduled');
        document.querySelector('#emailSchedTable tbody').innerHTML = rows.map(r => {
          const to = Array.isArray(r.to_addresses) ? r.to_addresses.join(', ') : JSON.stringify(r.to_addresses);
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td>${escapeHtml(to)}</td>
            <td>${escapeHtml(r.subject)}</td>
            <td>${new Date(r.scheduled_at).toLocaleString()}</td>
            <td>${escapeHtml(r.status)}</td>
            <td>${r.status === 'scheduled' ? `<button class="button secondary sched-cancel" data-id="${r.id}">Cancel</button>` : ''}</td>
          </tr>`;
        }).join('');
        document.getElementById('emailSchedErr').textContent = '';
        document.querySelectorAll('.sched-cancel').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/email/scheduled/${btn.dataset.id}`, { method: 'DELETE' });
              await loadScheduled();
            } catch (e) { document.getElementById('emailSchedErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailSchedErr').textContent = e.message; }
    };
    loadScheduled();

    // --- YARA Rules ---
    const loadYaraRules = async () => {
      try {
        const rows = await apiFetch('/admin/yara/rules');
        document.querySelector('#yaraRulesTable tbody').innerHTML = rows.map(r => {
          const scope = r.project_id ? `project:${r.project_id.substring(0,8)}` : 'global';
          const tags = (r.tags || []).map(t => `<span class="badge">${escapeHtml(t)}</span>`).join(' ');
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td><button class="button secondary yara-show" data-id="${r.id}">${escapeHtml(r.name)}</button></td>
            <td>${escapeHtml(r.description || '')}</td>
            <td>${tags}</td>
            <td>${escapeHtml(scope)}</td>
            <td>${new Date(r.created_at).toLocaleString()}</td>
            <td><button class="button secondary yara-del" data-id="${r.id}">Delete</button></td>
          </tr>`;
        }).join('');
        document.getElementById('yaraErr').textContent = '';
        document.querySelectorAll('.yara-del').forEach(btn => {
          btn.onclick = async () => {
            if (!confirm('Delete this YARA rule?')) return;
            try {
              await apiFetch(`/admin/yara/rules/${btn.dataset.id}`, { method: 'DELETE' });
              await loadYaraRules();
            } catch (e) { document.getElementById('yaraErr').textContent = e.message; }
          };
        });
        document.querySelectorAll('.yara-show').forEach(btn => {
          btn.onclick = async () => {
            try {
              const rule = await apiFetch(`/admin/yara/rules/${btn.dataset.id}`);
              document.getElementById('yaraRuleSource').textContent = rule.source;
              document.getElementById('yaraRuleDetail').style.display = 'block';
            } catch (e) { document.getElementById('yaraErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('yaraErr').textContent = e.message; }
    };
    loadYaraRules();

    document.getElementById('yaraScanLoad').onclick = async () => {
      const artifactId = document.getElementById('yaraScanArtifact').value.trim();
      if (!artifactId) return;
      try {
        const rows = await apiFetch(`/admin/yara/scan-results?artifact_id=${artifactId}`);
        document.querySelector('#yaraScanTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td>${escapeHtml(r.rule_name)}</td>
            <td>${r.match_count}</td>
            <td>${new Date(r.matched_at).toLocaleString()}</td>
          </tr>
        `).join('');
        document.getElementById('yaraScanErr').textContent = '';
      } catch (e) { document.getElementById('yaraScanErr').textContent = e.message; }
    };
  }
  async function renderKnowledge() {
    let projects = [];
    try { projects = await apiFetch('/projects'); } catch (_) {}

    renderShell(`
      <div class="card">
        <h3>Import URLs</h3>
        <div style="margin-bottom:16px;">
          <label>Project</label>
          <select id="urlProject" class="input" style="width:320px;">
            <option value="">-- select project --</option>
            ${projects.map(p => `<option value="${p.id}">${escapeHtml(p.name)}</option>`).join('')}
          </select>
        </div>
        <div style="margin-bottom:12px;">
          <label>Paste URLs (one per line)</label>
          <textarea id="urlTextarea" class="input" rows="6" style="width:100%;max-width:600px;font-family:monospace;font-size:13px;"
            placeholder="https://blog.example.com/post-1&#10;https://docs.example.com/guide"></textarea>
        </div>
        <button id="urlSubmitBtn" class="button">Import URLs</button>
        <span id="urlSubmitMsg" class="muted" style="margin-left:12px;"></span>
      </div>
      <div class="card">
        <div style="display:flex;align-items:center;gap:12px;margin-bottom:12px;">
          <h3 style="margin:0;">URL Ingest Queue</h3>
          <button id="urlRefreshBtn" class="button secondary">Refresh</button>
        </div>
        <div id="urlQueueErr" class="error"></div>
        <table id="urlQueueTable" class="table" style="font-size:13px;">
          <thead><tr><th>Status</th><th>URL</th><th>Title</th><th>Chunks</th><th>Error</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="card">
        <div style="display:flex;align-items:center;gap:12px;margin-bottom:12px;">
          <h3 style="margin:0;">Embed Queue</h3>
          <button id="eqRefreshBtn" class="button secondary">Refresh</button>
        </div>
        <div id="eqErr" class="error"></div>
        <table id="eqTable" class="table" style="font-size:13px;">
          <thead><tr><th>Status</th><th>Chunks Artifact</th><th>Embedded</th><th>Total</th><th>Source</th><th>Error</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
    `);

    let refreshTimer = null;

    // --- URL Ingest Queue ---
    async function loadQueue() {
      const pid = document.getElementById('urlProject').value;
      if (!pid) {
        document.getElementById('urlQueueTable').querySelector('tbody').innerHTML =
          '<tr><td colspan="6" class="muted">Select a project to see its queue.</td></tr>';
        return;
      }
      try {
        const items = await apiFetch('/projects/' + pid + '/url-ingest');
        const tbody = document.getElementById('urlQueueTable').querySelector('tbody');
        if (!items.length) {
          tbody.innerHTML = '<tr><td colspan="6" class="muted">No items in queue.</td></tr>';
        } else {
          tbody.innerHTML = items.map(r => {
            const statusColor = r.status === 'completed' ? 'color:#4a4' :
              r.status === 'failed' ? 'color:#c44' :
              r.status === 'processing' ? 'color:#48c' : 'color:#ca4';
            const urlShort = r.url.length > 50 ? r.url.slice(0, 47) + '...' : r.url;
            const actions = [];
            if (r.status === 'pending') actions.push('<button class="button secondary url-cancel-btn" data-id="' + r.id + '">Cancel</button>');
            if (r.status === 'failed') actions.push('<button class="button secondary url-retry-btn" data-id="' + r.id + '">Retry</button>');
            return '<tr>' +
              '<td style="' + statusColor + ';font-weight:600;">' + escapeHtml(r.status) + '</td>' +
              '<td title="' + r.url.replace(/"/g,'&quot;') + '">' + escapeHtml(urlShort) + '</td>' +
              '<td>' + escapeHtml(r.title || '-') + '</td>' +
              '<td>' + (r.chunk_count != null ? r.chunk_count : '-') + '</td>' +
              '<td style="color:#c44;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="' + (r.error_message || '').replace(/"/g,'&quot;') + '">' + escapeHtml(r.error_message || '') + '</td>' +
              '<td>' + actions.join(' ') + '</td>' +
              '</tr>';
          }).join('');
        }

        const hasPending = items.some(r => r.status === 'pending' || r.status === 'processing');
        if (hasPending && !refreshTimer) {
          refreshTimer = setInterval(loadQueue, 5000);
        } else if (!hasPending && refreshTimer) {
          clearInterval(refreshTimer);
          refreshTimer = null;
        }

        document.getElementById('urlQueueErr').textContent = '';
      } catch (e) {
        document.getElementById('urlQueueErr').textContent = e.message;
      }
    }

    document.getElementById('urlProject').onchange = () => {
      if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
      loadQueue();
      loadEmbedQueue();
    };

    document.getElementById('urlRefreshBtn').onclick = loadQueue;

    document.getElementById('urlSubmitBtn').onclick = async () => {
      const pid = document.getElementById('urlProject').value;
      const text = document.getElementById('urlTextarea').value;
      const msg = document.getElementById('urlSubmitMsg');
      if (!pid) { msg.textContent = 'Select a project first.'; return; }
      const urls = text.split('\n').map(s => s.trim()).filter(s => s.length > 0);
      if (!urls.length) { msg.textContent = 'Enter at least one URL.'; return; }
      try {
        msg.textContent = 'Submitting...';
        const result = await apiFetch('/projects/' + pid + '/url-ingest', {
          method: 'POST',
          body: JSON.stringify({ urls }),
        });
        msg.textContent = result.length + ' URL(s) enqueued.';
        document.getElementById('urlTextarea').value = '';
        loadQueue();
      } catch (e) { msg.textContent = 'Error: ' + e.message; }
    };

    document.getElementById('urlQueueTable').onclick = async (ev) => {
      const pid = document.getElementById('urlProject').value;
      if (!pid) return;
      const cancelBtn = ev.target.closest('.url-cancel-btn');
      const retryBtn = ev.target.closest('.url-retry-btn');
      if (cancelBtn) {
        try {
          await apiFetch('/projects/' + pid + '/url-ingest/' + cancelBtn.dataset.id, { method: 'DELETE' });
          loadQueue();
        } catch (e) { document.getElementById('urlQueueErr').textContent = e.message; }
      }
      if (retryBtn) {
        try {
          await apiFetch('/projects/' + pid + '/url-ingest/' + retryBtn.dataset.id + '/retry', { method: 'POST', body: '{}' });
          loadQueue();
        } catch (e) { document.getElementById('urlQueueErr').textContent = e.message; }
      }
    };

    // --- Embed Queue ---
    const loadEmbedQueue = async () => {
      try {
        const pid = document.getElementById('urlProject').value;
        const qs = pid ? '?project_id=' + pid : '';
        const rows = await apiFetch('/admin/embed-queue' + qs);
        const tbody = document.getElementById('eqTable').querySelector('tbody');
        if (!rows.length) {
          tbody.innerHTML = '<tr><td colspan="7" class="muted">No embed queue items.</td></tr>';
        } else {
          tbody.innerHTML = rows.map(r => {
            const statusColor = r.status === 'completed' ? 'color:#4a4' :
              r.status === 'failed' ? 'color:#c44' :
              r.status === 'processing' ? 'color:#48c' : 'color:#ca4';
            const actions = [];
            if (r.status === 'pending') actions.push('<button class="button secondary eq-cancel-btn" data-id="' + r.id + '">Cancel</button>');
            if (r.status === 'failed') actions.push('<button class="button secondary eq-retry-btn" data-id="' + r.id + '">Retry</button>');
            return '<tr>' +
              '<td style="' + statusColor + ';font-weight:600;">' + escapeHtml(r.status) + '</td>' +
              '<td class="muted">' + (r.chunks_artifact_id || '').substring(0, 8) + '</td>' +
              '<td>' + (r.chunks_embedded != null ? r.chunks_embedded : '-') + '</td>' +
              '<td>' + (r.chunk_count != null ? r.chunk_count : '-') + '</td>' +
              '<td>' + escapeHtml(r.tool_name || '-') + '</td>' +
              '<td style="color:#c44;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="' + (r.error_message || '').replace(/"/g,'&quot;') + '">' + escapeHtml(r.error_message || '') + '</td>' +
              '<td>' + actions.join(' ') + '</td>' +
              '</tr>';
          }).join('');
        }
        document.getElementById('eqErr').textContent = '';
      } catch (e) {
        document.getElementById('eqErr').textContent = e.message;
      }
    };

    document.getElementById('eqRefreshBtn').onclick = loadEmbedQueue;

    document.getElementById('eqTable').onclick = async (ev) => {
      const cancelBtn = ev.target.closest('.eq-cancel-btn');
      const retryBtn = ev.target.closest('.eq-retry-btn');
      if (cancelBtn) {
        try {
          await apiFetch('/admin/embed-queue/' + cancelBtn.dataset.id, { method: 'DELETE' });
          loadEmbedQueue();
        } catch (e) { document.getElementById('eqErr').textContent = e.message; }
      }
      if (retryBtn) {
        try {
          await apiFetch('/admin/embed-queue/' + retryBtn.dataset.id + '/retry', { method: 'POST', body: '{}' });
          loadEmbedQueue();
        } catch (e) { document.getElementById('eqErr').textContent = e.message; }
      }
    };

    loadQueue();
    loadEmbedQueue();
  }

  async function renderYaraPage() {
    renderShell(`
      <div class="card">
        <h3>YARA Rules</h3>
        <div id="yaraErr" class="error"></div>
        <table class="table" id="yaraRulesTable">
          <thead><tr><th>ID</th><th>Name</th><th>Description</th><th>Tags</th><th>Scope</th><th>Created</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div id="yaraRuleDetail" style="display:none; margin-top:12px;">
          <h4>Rule Source</h4>
          <pre id="yaraRuleSource" style="max-height:400px; overflow:auto; background:var(--bg-secondary); padding:8px; border-radius:4px;"></pre>
        </div>
      </div>
      <div class="card">
        <h3>Scan Results</h3>
        <div class="row" style="margin-bottom:10px;">
          <input id="yaraScanArtifact" class="input" placeholder="Artifact UUID" />
          <button id="yaraScanLoad" class="button">Load Results</button>
        </div>
        <div id="yaraScanErr" class="error"></div>
        <table class="table" id="yaraScanTable">
          <thead><tr><th>Rule</th><th>Matches</th><th>Matched At</th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
    `);

    const loadYaraRules = async () => {
      try {
        const rows = await apiFetch('/admin/yara/rules');
        document.querySelector('#yaraRulesTable tbody').innerHTML = rows.map(r => {
          const scope = r.project_id ? `project:${r.project_id.substring(0,8)}` : 'global';
          const tags = (r.tags || []).map(t => `<span class="badge">${escapeHtml(t)}</span>`).join(' ');
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td><button class="button secondary yara-show" data-id="${r.id}">${escapeHtml(r.name)}</button></td>
            <td>${escapeHtml(r.description || '')}</td>
            <td>${tags}</td>
            <td>${escapeHtml(scope)}</td>
            <td>${new Date(r.created_at).toLocaleString()}</td>
            <td><button class="button secondary yara-del" data-id="${r.id}">Delete</button></td>
          </tr>`;
        }).join('');
        document.getElementById('yaraErr').textContent = '';
        document.querySelectorAll('.yara-del').forEach(btn => {
          btn.onclick = async () => {
            if (!confirm('Delete this YARA rule?')) return;
            try {
              await apiFetch(`/admin/yara/rules/${btn.dataset.id}`, { method: 'DELETE' });
              await loadYaraRules();
            } catch (e) { document.getElementById('yaraErr').textContent = e.message; }
          };
        });
        document.querySelectorAll('.yara-show').forEach(btn => {
          btn.onclick = async () => {
            try {
              const rule = await apiFetch(`/admin/yara/rules/${btn.dataset.id}`);
              document.getElementById('yaraRuleSource').textContent = rule.source;
              document.getElementById('yaraRuleDetail').style.display = 'block';
            } catch (e) { document.getElementById('yaraErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('yaraErr').textContent = e.message; }
    };
    loadYaraRules();

    document.getElementById('yaraScanLoad').onclick = async () => {
      const artifactId = document.getElementById('yaraScanArtifact').value.trim();
      if (!artifactId) return;
      try {
        const rows = await apiFetch(`/admin/yara/scan-results?artifact_id=${artifactId}`);
        document.querySelector('#yaraScanTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td>${escapeHtml(r.rule_name)}</td>
            <td>${r.match_count}</td>
            <td>${new Date(r.matched_at).toLocaleString()}</td>
          </tr>
        `).join('');
        document.getElementById('yaraScanErr').textContent = '';
      } catch (e) { document.getElementById('yaraScanErr').textContent = e.message; }
    };
  }

  async function renderWebRulesPage() {
    renderShell(`
      <div class="card">
        <h3>Web URL Rules</h3>
        <div id="webRulesErr" class="error"></div>
        <table class="table" id="webRulesTable">
          <thead><tr><th>ID</th><th>Type</th><th>Scope</th><th>Pattern Type</th><th>Pattern</th><th>Description</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <h4 style="margin-top:12px;">Add Rule</h4>
        <div class="row" style="margin-bottom:8px;">
          <select id="wrScope" class="input"><option value="global">Global</option><option value="project">Project</option></select>
          <input id="wrProjectId" class="input" placeholder="Project UUID (if project scope)" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <label><input type="radio" name="wrType" value="block" checked /> Block</label>
          <label style="margin-left:12px;"><input type="radio" name="wrType" value="allow" /> Allow</label>
        </div>
        <div class="row" style="margin-bottom:8px;">
          <select id="wrPatternType" class="input">
            <option value="domain">domain</option>
            <option value="domain_suffix">domain_suffix</option>
            <option value="url_prefix">url_prefix</option>
            <option value="url_regex">url_regex</option>
            <option value="ip_cidr">ip_cidr</option>
          </select>
          <input id="wrPattern" class="input" placeholder="Pattern value" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <input id="wrDesc" class="input" placeholder="Description (optional)" />
          <button id="wrAdd" class="button">Add Rule</button>
        </div>
      </div>
      <div class="card">
        <h3>Country Blocks</h3>
        <div id="countryErr" class="error"></div>
        <table class="table" id="countryTable">
          <thead><tr><th>Country Code</th><th>Added</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div class="row" style="margin-top:8px;">
          <input id="countryCode" class="input" placeholder="Country code (e.g. CN, RU)" style="width:120px;" />
          <button id="countryAdd" class="button">Block Country</button>
        </div>
      </div>
    `);

    // --- Web URL Rules ---
    const loadWebRules = async () => {
      try {
        const rows = await apiFetch('/web-rules');
        document.querySelector('#webRulesTable tbody').innerHTML = rows.map(r => {
          const scope = r.project_id ? `project:${r.project_id.substring(0,8)}` : 'global';
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td>${escapeHtml(r.rule_type)}</td>
            <td>${escapeHtml(scope)}</td>
            <td>${escapeHtml(r.pattern_type)}</td>
            <td>${escapeHtml(r.pattern)}</td>
            <td>${escapeHtml(r.description || '')}</td>
            <td><button class="button secondary wr-del" data-id="${r.id}">Delete</button></td>
          </tr>`;
        }).join('');
        document.getElementById('webRulesErr').textContent = '';
        document.querySelectorAll('.wr-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/web-rules/${btn.dataset.id}`, { method: 'DELETE' });
              await loadWebRules();
            } catch (e) { document.getElementById('webRulesErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('webRulesErr').textContent = e.message; }
    };
    loadWebRules();

    document.getElementById('wrAdd').onclick = async () => {
      const scope = document.getElementById('wrScope').value;
      const projectId = document.getElementById('wrProjectId').value.trim() || null;
      const ruleType = document.querySelector('input[name="wrType"]:checked').value;
      const patternType = document.getElementById('wrPatternType').value;
      const pattern = document.getElementById('wrPattern').value.trim();
      const desc = document.getElementById('wrDesc').value.trim() || null;
      if (!pattern) return;
      try {
        await apiFetch('/web-rules', {
          method: 'POST',
          body: JSON.stringify({ scope, project_id: projectId, rule_type: ruleType, pattern_type: patternType, pattern, description: desc }),
        });
        document.getElementById('wrPattern').value = '';
        document.getElementById('wrDesc').value = '';
        await loadWebRules();
      } catch (e) { document.getElementById('webRulesErr').textContent = e.message; }
    };

    // --- Country Blocks ---
    const loadCountries = async () => {
      try {
        const rows = await apiFetch('/web-rules/countries');
        document.querySelector('#countryTable tbody').innerHTML = rows.length
          ? rows.map(r => `<tr>
              <td>${escapeHtml(r.country_code)}</td>
              <td>${new Date(r.created_at).toLocaleString()}</td>
              <td><button class="button secondary country-del" data-code="${escapeHtml(r.country_code)}">Remove</button></td>
            </tr>`).join('')
          : '<tr><td colspan="3" class="muted">No country blocks configured.</td></tr>';
        document.getElementById('countryErr').textContent = '';
        document.querySelectorAll('.country-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/web-rules/countries/${btn.dataset.code}`, { method: 'DELETE' });
              await loadCountries();
            } catch (e) { document.getElementById('countryErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('countryErr').textContent = e.message; }
    };
    loadCountries();

    document.getElementById('countryAdd').onclick = async () => {
      const code = document.getElementById('countryCode').value.trim().toUpperCase();
      if (!code || code.length !== 2) return;
      try {
        await apiFetch('/web-rules/countries', {
          method: 'POST',
          body: JSON.stringify({ country_code: code }),
        });
        document.getElementById('countryCode').value = '';
        await loadCountries();
      } catch (e) { document.getElementById('countryErr').textContent = e.message; }
    };
  }

  async function renderEmailAdminPage() {
    renderShell(`
      <div class="card">
        <h3>Email Credentials</h3>
        <div id="emailCredsErr" class="error"></div>
        <table class="table" id="emailCredsTable">
          <thead><tr><th>ID</th><th>Provider</th><th>Email</th><th>Default</th><th>Created</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="card">
        <h3>Tone Presets</h3>
        <div id="emailTonesErr" class="error"></div>
        <table class="table" id="emailTonesTable">
          <thead><tr><th>Name</th><th>Description</th><th>Builtin</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <div class="row" style="margin-top:8px;">
          <input id="toneName" class="input" placeholder="Name" />
          <input id="toneDesc" class="input" placeholder="Description" />
          <input id="toneInstr" class="input" placeholder="System instruction" />
          <button id="toneAdd" class="button">Add Tone</button>
        </div>
      </div>
      <div class="card">
        <h3>Recipient Rules</h3>
        <div id="emailRulesErr" class="error"></div>
        <table class="table" id="emailRulesTable">
          <thead><tr><th>ID</th><th>Type</th><th>Target Type</th><th>Pattern</th><th>Scope</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <h4 style="margin-top:12px;">Add Rule</h4>
        <div class="row" style="margin-bottom:8px;">
          <label><input type="radio" name="erType" value="block" checked /> Block</label>
          <label style="margin-left:12px;"><input type="radio" name="erType" value="allow" /> Allow</label>
        </div>
        <div class="row" style="margin-bottom:8px;">
          <select id="erPatternType" class="input">
            <option value="email">email</option>
            <option value="domain">domain</option>
            <option value="domain_suffix">domain_suffix</option>
          </select>
          <input id="erPattern" class="input" placeholder="Target (e.g. user@example.com)" />
        </div>
        <div class="row" style="margin-bottom:8px;">
          <input id="erDesc" class="input" placeholder="Description (optional)" />
          <button id="erAdd" class="button">Add Rule</button>
        </div>
      </div>
      <div class="card">
        <h3>Scheduled Emails</h3>
        <div id="emailSchedErr" class="error"></div>
        <table class="table" id="emailSchedTable">
          <thead><tr><th>ID</th><th>To</th><th>Subject</th><th>Scheduled</th><th>Status</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
    `);

    // --- Credentials ---
    const loadEmailCreds = async () => {
      try {
        const rows = await apiFetch('/admin/email/credentials');
        document.querySelector('#emailCredsTable tbody').innerHTML = rows.length
          ? rows.map(r => `
            <tr>
              <td class="muted">${r.id.substring(0,8)}</td>
              <td>${escapeHtml(r.provider)}</td>
              <td>${escapeHtml(r.email_address)}</td>
              <td>${r.is_default ? 'yes' : ''}</td>
              <td>${new Date(r.created_at).toLocaleString()}</td>
              <td><button class="button secondary cred-del" data-id="${r.id}">Delete</button></td>
            </tr>
          `).join('')
          : '<tr><td colspan="6" class="muted">No email credentials configured.</td></tr>';
        document.getElementById('emailCredsErr').textContent = '';
        document.querySelectorAll('.cred-del').forEach(btn => {
          btn.onclick = async () => {
            if (!confirm('Remove this email credential?')) return;
            try {
              await apiFetch(`/admin/email/credentials/${btn.dataset.id}`, { method: 'DELETE' });
              await loadEmailCreds();
            } catch (e) { document.getElementById('emailCredsErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailCredsErr').textContent = e.message; }
    };
    loadEmailCreds();

    // --- Tones ---
    const loadTones = async () => {
      try {
        const rows = await apiFetch('/admin/email/tones');
        document.querySelector('#emailTonesTable tbody').innerHTML = rows.map(r => `
          <tr>
            <td>${escapeHtml(r.name)}</td>
            <td>${escapeHtml(r.description || '')}</td>
            <td>${r.is_builtin ? 'yes' : ''}</td>
            <td>${r.is_builtin ? '' : `<button class="button secondary tone-del" data-name="${escapeHtml(r.name)}">Delete</button>`}</td>
          </tr>
        `).join('');
        document.getElementById('emailTonesErr').textContent = '';
        document.querySelectorAll('.tone-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/email/tones/${encodeURIComponent(btn.dataset.name)}`, { method: 'DELETE' });
              await loadTones();
            } catch (e) { document.getElementById('emailTonesErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailTonesErr').textContent = e.message; }
    };
    loadTones();

    document.getElementById('toneAdd').onclick = async () => {
      const name = document.getElementById('toneName').value.trim();
      const desc = document.getElementById('toneDesc').value.trim();
      const instr = document.getElementById('toneInstr').value.trim();
      if (!name || !instr) return;
      try {
        await apiFetch('/admin/email/tones', {
          method: 'POST',
          body: JSON.stringify({ name, description: desc || null, system_instruction: instr }),
        });
        document.getElementById('toneName').value = '';
        document.getElementById('toneDesc').value = '';
        document.getElementById('toneInstr').value = '';
        await loadTones();
      } catch (e) { document.getElementById('emailTonesErr').textContent = e.message; }
    };

    // --- Recipient Rules ---
    const loadEmailRules = async () => {
      try {
        const rows = await apiFetch('/admin/email/rules');
        document.querySelector('#emailRulesTable tbody').innerHTML = rows.map(r => {
          const scope = r.project_id ? `project:${r.project_id.substring(0,8)}` : 'global';
          return `<tr>
            <td class="muted">${r.id.substring(0,8)}</td>
            <td>${escapeHtml(r.rule_type)}</td>
            <td>${escapeHtml(r.pattern_type)}</td>
            <td>${escapeHtml(r.pattern)}</td>
            <td>${escapeHtml(scope)}</td>
            <td><button class="button secondary er-del" data-id="${r.id}">Delete</button></td>
          </tr>`;
        }).join('');
        document.getElementById('emailRulesErr').textContent = '';
        document.querySelectorAll('.er-del').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/email/rules/${btn.dataset.id}`, { method: 'DELETE' });
              await loadEmailRules();
            } catch (e) { document.getElementById('emailRulesErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailRulesErr').textContent = e.message; }
    };
    loadEmailRules();

    document.getElementById('erAdd').onclick = async () => {
      const ruleType = document.querySelector('input[name="erType"]:checked').value;
      const patternType = document.getElementById('erPatternType').value;
      const pattern = document.getElementById('erPattern').value.trim();
      const desc = document.getElementById('erDesc').value.trim() || null;
      if (!pattern) return;
      try {
        await apiFetch('/admin/email/rules', {
          method: 'POST',
          body: JSON.stringify({ scope: 'global', project_id: null, rule_type: ruleType, pattern_type: patternType, pattern, description: desc }),
        });
        document.getElementById('erPattern').value = '';
        document.getElementById('erDesc').value = '';
        await loadEmailRules();
      } catch (e) { document.getElementById('emailRulesErr').textContent = e.message; }
    };

    // --- Scheduled Emails ---
    const loadScheduled = async () => {
      try {
        const rows = await apiFetch('/admin/email/scheduled');
        document.querySelector('#emailSchedTable tbody').innerHTML = rows.length
          ? rows.map(r => {
              const to = Array.isArray(r.to_addresses) ? r.to_addresses.join(', ') : JSON.stringify(r.to_addresses);
              return `<tr>
                <td class="muted">${r.id.substring(0,8)}</td>
                <td>${escapeHtml(to)}</td>
                <td>${escapeHtml(r.subject)}</td>
                <td>${new Date(r.scheduled_at).toLocaleString()}</td>
                <td>${escapeHtml(r.status)}</td>
                <td>${r.status === 'scheduled' ? `<button class="button secondary sched-cancel" data-id="${r.id}">Cancel</button>` : ''}</td>
              </tr>`;
            }).join('')
          : '<tr><td colspan="6" class="muted">No scheduled emails.</td></tr>';
        document.getElementById('emailSchedErr').textContent = '';
        document.querySelectorAll('.sched-cancel').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/admin/email/scheduled/${btn.dataset.id}`, { method: 'DELETE' });
              await loadScheduled();
            } catch (e) { document.getElementById('emailSchedErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('emailSchedErr').textContent = e.message; }
    };
    loadScheduled();
  }

  async function renderNotificationsPage() {
    renderShell(`
      <div class="card">
        <h3>Notification Channels</h3>
        <div class="row" style="margin-bottom:8px;">
          <label>Project:</label>
          <select id="notifProjectSelect" class="input" style="width:300px;"></select>
        </div>
        <div id="notifChannelsErr" class="error"></div>
        <table class="table" id="notifChannelsTable">
          <thead><tr><th>Name</th><th>Type</th><th>Enabled</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <details style="margin-top:8px;">
          <summary style="cursor:pointer;">+ Add Channel</summary>
          <div class="row" style="margin-top:8px;flex-wrap:wrap;gap:8px;">
            <input id="notifChName" class="input" placeholder="Channel name" style="width:150px;" />
            <select id="notifChType" class="input" style="width:120px;">
              <option value="webhook">webhook</option>
              <option value="email">email</option>
              <option value="matrix">matrix</option>
              <option value="webdav">webdav</option>
            </select>
            <div id="notifChConfigFields" style="display:flex;gap:8px;flex-wrap:wrap;"></div>
            <button id="notifChAddBtn" class="btn">Add</button>
          </div>
        </details>
      </div>
      <div class="card">
        <h3>Recent Notifications</h3>
        <div id="notifQueueErr" class="error"></div>
        <table class="table" id="notifQueueTable">
          <thead><tr><th>Status</th><th>Channel</th><th>Subject</th><th>Time</th><th>Error</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
      </div>
    `);

    const projSelect = document.getElementById('notifProjectSelect');
    const chTypeSelect = document.getElementById('notifChType');
    const configFieldsDiv = document.getElementById('notifChConfigFields');

    // Load projects
    try {
      const projects = await apiFetch('/projects');
      projects.forEach(p => {
        _ndaProjectCache[p.id] = !!p.nda;
        const opt = document.createElement('option');
        opt.value = p.id;
        opt.textContent = p.name + (p.nda ? ' [NDA]' : '');
        projSelect.appendChild(opt);
      });
    } catch (e) {
      document.getElementById('notifChannelsErr').textContent = e.message;
      return;
    }

    function renderConfigFields() {
      const t = chTypeSelect.value;
      let html = '';
      if (t === 'webhook') {
        html = '<input id="notifCfgUrl" class="input" placeholder="https://hooks.example.com/..." style="width:300px;" />';
      } else if (t === 'email') {
        html = '<input id="notifCfgTo" class="input" placeholder="user@example.com (comma-sep)" style="width:250px;" />' +
               '<input id="notifCfgCredId" class="input" placeholder="credential_id (UUID)" style="width:250px;" />';
      } else if (t === 'matrix') {
        html = '<input id="notifCfgHomeserver" class="input" placeholder="https://matrix.org" style="width:200px;" />' +
               '<input id="notifCfgRoomId" class="input" placeholder="!room:matrix.org" style="width:200px;" />' +
               '<input id="notifCfgToken" class="input" placeholder="Access token" style="width:200px;" />';
      } else if (t === 'webdav') {
        html = '<input id="notifCfgDavUrl" class="input" placeholder="https://nextcloud.example.com/..." style="width:300px;" />' +
               '<input id="notifCfgDavUser" class="input" placeholder="Username" style="width:120px;" />' +
               '<input id="notifCfgDavPass" class="input" placeholder="Password" type="password" style="width:120px;" />';
      }
      configFieldsDiv.innerHTML = html;
    }
    chTypeSelect.onchange = renderConfigFields;
    renderConfigFields();

    function buildConfig() {
      const t = chTypeSelect.value;
      if (t === 'webhook') {
        return { url: (document.getElementById('notifCfgUrl') || {}).value || '' };
      } else if (t === 'email') {
        const to = ((document.getElementById('notifCfgTo') || {}).value || '').split(',').map(s => s.trim()).filter(Boolean);
        return { to, credential_id: (document.getElementById('notifCfgCredId') || {}).value || '' };
      } else if (t === 'matrix') {
        return {
          homeserver: (document.getElementById('notifCfgHomeserver') || {}).value || '',
          room_id: (document.getElementById('notifCfgRoomId') || {}).value || '',
          access_token: (document.getElementById('notifCfgToken') || {}).value || '',
        };
      } else if (t === 'webdav') {
        return {
          url: (document.getElementById('notifCfgDavUrl') || {}).value || '',
          username: (document.getElementById('notifCfgDavUser') || {}).value || '',
          password: (document.getElementById('notifCfgDavPass') || {}).value || '',
        };
      }
      return {};
    }

    async function loadChannels() {
      const pid = projSelect.value;
      if (!pid) return;
      try {
        const channels = await apiFetch(`/projects/${pid}/notification-channels`);
        const tbody = document.querySelector('#notifChannelsTable tbody');
        tbody.innerHTML = channels.map(ch => `
          <tr>
            <td>${ch.name}</td>
            <td>${ch.channel_type}</td>
            <td>${ch.enabled ? 'yes' : 'no'}</td>
            <td>
              <button class="btn btn-sm" data-test="${ch.id}">Test</button>
              <button class="btn btn-sm btn-danger" data-del="${ch.id}">Del</button>
            </td>
          </tr>
        `).join('');
        tbody.querySelectorAll('[data-test]').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/projects/${pid}/notification-channels/${btn.dataset.test}/test`, { method: 'POST' });
              await loadQueue();
            } catch (e) { document.getElementById('notifChannelsErr').textContent = e.message; }
          };
        });
        tbody.querySelectorAll('[data-del]').forEach(btn => {
          btn.onclick = async () => {
            if (!confirm('Delete this channel?')) return;
            try {
              await apiFetch(`/projects/${pid}/notification-channels/${btn.dataset.del}`, { method: 'DELETE' });
              await loadChannels();
            } catch (e) { document.getElementById('notifChannelsErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('notifChannelsErr').textContent = e.message; }
    }

    async function loadQueue() {
      const pid = projSelect.value;
      if (!pid) return;
      try {
        const items = await apiFetch(`/projects/${pid}/notifications`);
        const tbody = document.querySelector('#notifQueueTable tbody');
        tbody.innerHTML = items.map(it => {
          const ago = ((Date.now() - new Date(it.created_at).getTime()) / 60000).toFixed(0);
          const subj = it.subject.length > 40 ? it.subject.slice(0, 37) + '...' : it.subject;
          const actions = [];
          if (it.status === 'pending') actions.push(`<button class="btn btn-sm" data-cancel="${it.id}">Cancel</button>`);
          if (it.status === 'failed') actions.push(`<button class="btn btn-sm" data-retry="${it.id}">Retry</button>`);
          return `<tr>
            <td><span class="badge badge-${it.status === 'completed' ? 'ok' : it.status === 'failed' ? 'err' : 'info'}">${it.status}</span></td>
            <td>${it.channel_id.slice(0, 8)}...</td>
            <td>${subj}</td>
            <td>${ago}m ago</td>
            <td>${it.error_message || ''}</td>
            <td>${actions.join(' ')}</td>
          </tr>`;
        }).join('');
        tbody.querySelectorAll('[data-cancel]').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/projects/${pid}/notifications/${btn.dataset.cancel}`, { method: 'DELETE' });
              await loadQueue();
            } catch (e) { document.getElementById('notifQueueErr').textContent = e.message; }
          };
        });
        tbody.querySelectorAll('[data-retry]').forEach(btn => {
          btn.onclick = async () => {
            try {
              await apiFetch(`/projects/${pid}/notifications/${btn.dataset.retry}/retry`, { method: 'POST' });
              await loadQueue();
            } catch (e) { document.getElementById('notifQueueErr').textContent = e.message; }
          };
        });
      } catch (e) { document.getElementById('notifQueueErr').textContent = e.message; }
    }

    projSelect.onchange = () => {
      loadChannels(); loadQueue();
      checkProjectNda(projSelect.value).then(nda => applyNdaTint(nda));
    };

    document.getElementById('notifChAddBtn').onclick = async () => {
      const pid = projSelect.value;
      if (!pid) return;
      const name = document.getElementById('notifChName').value.trim();
      if (!name) return;
      try {
        await apiFetch(`/projects/${pid}/notification-channels`, {
          method: 'POST',
          body: JSON.stringify({
            name,
            channel_type: chTypeSelect.value,
            config: buildConfig(),
          }),
        });
        document.getElementById('notifChName').value = '';
        await loadChannels();
      } catch (e) { document.getElementById('notifChannelsErr').textContent = e.message; }
    };

    if (projSelect.value) {
      loadChannels(); loadQueue();
      checkProjectNda(projSelect.value).then(nda => applyNdaTint(nda));
    }
  }

  async function renderRoute() {
    if (!state.apiKey) {
      renderLogin();
      return;
    }

    if (state.route.startsWith('#/thread/')) {
      const threadId = state.route.split('/')[2];
      if (threadId) {
        await renderThreadDetail(threadId);
        return;
      }
    }

    if (state.route.startsWith('#/threads/')) {
      const projectId = state.route.split('/')[2];
      if (projectId) {
        await renderThreads(projectId);
        return;
      }
    }

    if (state.route.startsWith('#/threads')) {
      await renderThreadsHome();
      return;
    }

    if (state.route.startsWith('#/hooks/')) {
      const parts = state.route.split('/');
      const projectId = parts[2];
      const hookId = parts[3];
      if (projectId && hookId && hookId !== 'new') {
        await renderHookDetail(projectId, hookId);
        return;
      }
      if (projectId) {
        await renderHooks(projectId);
        return;
      }
    }

    if (state.route.startsWith('#/workflows/')) {
      const name = state.route.split('/')[2];
      if (name) {
        await renderWorkflowDetail(name);
        return;
      }
    }

    if (state.route.startsWith('#/workflows')) {
      await renderWorkflowsHome();
      return;
    }

    if (state.route.startsWith('#/audit')) {
      await renderAudit();
      return;
    }

    if (state.route.startsWith('#/search')) {
      await renderSearch();
      return;
    }

    if (state.route.startsWith('#/knowledge')) {
      await renderKnowledge();
      return;
    }

    if (state.route.startsWith('#/yara')) {
      await renderYaraPage();
      return;
    }

    if (state.route.startsWith('#/web-rules')) {
      await renderWebRulesPage();
      return;
    }

    if (state.route.startsWith('#/email-admin')) {
      await renderEmailAdminPage();
      return;
    }

    if (state.route.startsWith('#/notifications')) {
      await renderNotificationsPage();
      return;
    }

    if (state.route.startsWith('#/admin')) {
      await renderAdmin();
      return;
    }

    if (state.route.startsWith('#/artifacts')) {
      await renderArtifacts();
      return;
    }

    if (state.route.startsWith('#/plugins')) {
      await renderPlugins();
      return;
    }

    if (state.route.startsWith('#/tools')) {
      await renderTools();
      return;
    }

    if (state.route.startsWith('#/agents/')) {
      const name = state.route.split('/')[2];
      if (name) {
        await renderAgentDetail(name);
        return;
      }
    }

    if (state.route.startsWith('#/agents')) {
      await renderAgents();
      return;
    }

    if (state.route.startsWith('#/projects/')) {
      const parts = state.route.split('/');
      const projectId = parts[2];
      if (parts.length >= 5 && parts[3] === 'artifacts') {
        const artifactId = parts[4];
        if (projectId && artifactId) {
          await renderArtifactDetail(projectId, artifactId);
          return;
        }
      }
      if (projectId) {
        await renderProjectDetail(projectId);
        return;
      }
    }

    if (state.route.startsWith('#/projects')) {
      await renderProjects();
      return;
    }

    await renderDashboard();
  }

  window.addEventListener('hashchange', () => {
    state.route = location.hash || '#/dashboard';
    render();
  });

  function render() {
    state.route = location.hash || '#/dashboard';
    applyTheme(getSessionSettings().theme);
    renderRoute();
  }

  render();
})();
