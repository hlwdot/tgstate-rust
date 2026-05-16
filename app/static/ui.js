// Toast Notification
const Toast = {
    show(message, type = 'success') {
        const container = document.getElementById('toast-container') || this.createContainer();
        
        const toast = document.createElement('div');
        toast.className = `toast toast-${type}`;
        toast.style.cssText = `
            background: var(--bg-solid); color: var(--text-primary);
            padding: 12px 16px; border-radius: var(--sharp-corner); margin-top: 12px;
            box-shadow: var(--shadow-float); font-size: 13px;
            display: flex; align-items: center; gap: 8px;
            opacity: 0; transition: opacity 0.2s;
            border: 1px solid var(--border-color);
            border-left: 4px solid ${type === 'error' ? 'var(--danger-color)' : 'var(--success-color)'};
        `;
        
        // Icon
        const icon = type === 'error' 
            ? '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--danger-color)" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line></svg>'
            : '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--success-color)" stroke-width="2"><polyline points="20 6 9 17 4 12"></polyline></svg>';
            
        toast.insertAdjacentHTML('beforeend', icon);
        const text = document.createElement('span');
        text.textContent = String(message);
        toast.appendChild(text);
        
        container.appendChild(toast);
        
        // Animate in
        requestAnimationFrame(() => {
            toast.style.opacity = '1';
        });
        
        // Auto remove
        setTimeout(() => {
            toast.style.opacity = '0';
            setTimeout(() => toast.remove(), 300);
        }, 3000);
    },
    
    createContainer() {
        const div = document.createElement('div');
        div.id = 'toast-container';
        div.style.cssText = `
            position: fixed; top: 20px; left: 50%; transform: translateX(-50%);
            z-index: 2100; display: flex; flex-direction: column; align-items: center;
        `;
        document.body.appendChild(div);
        return div;
    }
};

// Modal System
const Modal = {
    init() {
        if (document.getElementById('app-modal-overlay')) return;
        
        const overlay = document.createElement('div');
        overlay.id = 'app-modal-overlay';
        overlay.style.cssText = `
            position: fixed; top: 0; left: 0; right: 0; bottom: 0;
            background: rgba(0, 0, 0, 0.5); z-index: 2000;
            display: none; align-items: center; justify-content: center;
            opacity: 0; transition: opacity 0.2s;
        `;
        
        const card = document.createElement('div');
        card.id = 'app-modal-card';
        card.style.cssText = `
            background: var(--bg-solid); width: 90%; max-width: 400px;
            border: 1px solid var(--border-color);
            border-radius: var(--sharp-corner); padding: 24px;
            box-shadow: var(--shadow-md); transform: scale(0.95);
            transition: transform 0.2s;
        `;
        
        card.innerHTML = `
            <h3 id="modal-title" style="font-size: 18px; font-weight: 700; margin: 0 0 12px;"></h3>
            <p id="modal-msg" style="color: var(--text-secondary); font-size: 14px; margin-bottom: 24px; line-height: 1.5;"></p>
            <div style="display: flex; gap: 12px; justify-content: flex-end;">
                <button id="modal-cancel" class="btn btn-secondary" style="flex: 1;">Cancel</button>
                <button id="modal-confirm" class="btn btn-primary" style="flex: 1;">Confirm</button>
            </div>
        `;
        
        overlay.appendChild(card);
        document.body.appendChild(overlay);
        
        this.overlay = overlay;
        this.card = card;
        this.titleEl = card.querySelector('#modal-title');
        this.msgEl = card.querySelector('#modal-msg');
        this.cancelBtn = card.querySelector('#modal-cancel');
        this.confirmBtn = card.querySelector('#modal-confirm');
    },
    
    confirm(title, message) {
        this.init();
        return new Promise((resolve) => {
            this.titleEl.textContent = title;
            this.msgEl.textContent = message;
            
            this.overlay.style.display = 'flex';
            // Force reflow
            this.overlay.offsetHeight;
            this.overlay.style.opacity = '1';
            this.card.style.transform = 'scale(1)';
            
            const close = (result) => {
                this.overlay.style.opacity = '0';
                this.card.style.transform = 'scale(0.95)';
                setTimeout(() => {
                    this.overlay.style.display = 'none';
                    resolve(result);
                }, 200);
            };
            
            this.confirmBtn.onclick = () => close(true);
            this.cancelBtn.onclick = () => close(false);
            this.overlay.onclick = (e) => {
                if (e.target === this.overlay) close(false);
            };
        });
    }
};

// Shared utilities
const Utils = {
    async copy(text) {
        // Convert relative download paths to absolute URLs before copying.
        if (text.startsWith('/')) {
            text = window.location.origin + text;
        }

        const successToast = () => Toast.show('Copied to clipboard');
        const failToast = () => Toast.show('Copy failed. Copy manually instead', 'error');

        // Prefer the async Clipboard API when available.
        if (navigator.clipboard && navigator.clipboard.writeText) {
            try {
                await navigator.clipboard.writeText(text);
                successToast();
                return true;
            } catch (err) {
                console.warn('Clipboard API failed, trying fallback...', err);
            }
        }

        // Fallback: document.execCommand('copy')
        try {
            const textarea = document.createElement('textarea');
            textarea.value = text;
            // Keep the textarea rendered so selection works on older browsers.
            textarea.style.position = 'fixed';
            textarea.style.left = '0';
            textarea.style.top = '0';
            textarea.style.opacity = '0.01';
            textarea.style.pointerEvents = 'none';
            textarea.setAttribute('readonly', '');
            
            document.body.appendChild(textarea);
            textarea.focus();
            textarea.select();
            
            const successful = document.execCommand('copy');
            document.body.removeChild(textarea);
            
            if (successful) {
                successToast();
                return true;
            }
        } catch (fallbackErr) {
            console.error('Fallback copy failed:', fallbackErr);
        }
        
        // Final fallback: Modal with text
        try {
            const modal = document.createElement('div');
            modal.style.cssText = `
                position: fixed; top: 0; left: 0; right: 0; bottom: 0;
                background: rgba(0,0,0,0.5); z-index: 9999;
                display: flex; align-items: center; justify-content: center;
            `;
            const content = document.createElement('div');
            content.style.cssText = `
                background: var(--bg-solid); padding: 24px; border-radius: var(--sharp-corner);
                border: 1px solid var(--border-color);
                width: 90%; max-width: 320px; text-align: center;
                box-shadow: var(--shadow-float);
            `;
            const title = document.createElement('p');
            title.textContent = 'Copy failed. Copy manually:';
            title.style.cssText = 'margin-bottom: 12px; font-weight: 700; color: var(--text-primary);';

            const textWrap = document.createElement('div');
            textWrap.style.cssText = 'background: var(--bg-body); padding: 8px; border-radius: 6px; margin-bottom: 16px; border: 1px solid var(--border-color);';

            const textValue = document.createElement('div');
            textValue.textContent = text;
            textValue.style.cssText = 'word-break: break-all; font-family: monospace; font-size: 13px; color: var(--text-primary); user-select: text;';
            textWrap.appendChild(textValue);

            const closeButton = document.createElement('button');
            closeButton.className = 'btn btn-primary';
            closeButton.style.width = '100%';
            closeButton.textContent = 'Close';

            content.appendChild(title);
            content.appendChild(textWrap);
            content.appendChild(closeButton);
            modal.appendChild(content);
            document.body.appendChild(modal);
            
            const close = () => modal.remove();
            
            closeButton.onclick = close;
            modal.onclick = (e) => { if(e.target === modal) close(); };
        } catch (e) {}
        
        return false;
    },
    
    setLoading(btn, isLoading) {
        if (!btn) return;
        if (isLoading) {
            btn.dataset.originalText = btn.innerHTML;
            btn.innerHTML = `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="spin"><path d="M21 12a9 9 0 1 1-6.219-8.56"></path></svg> Working...`;
            btn.classList.add('loading');
            btn.disabled = true;
        } else {
            btn.innerHTML = btn.dataset.originalText || btn.innerHTML;
            btn.classList.remove('loading');
            btn.disabled = false;
        }
    }
};

// File link copy helper.
window.copyLink = (shortId, fileId, filename) => {
    const id = (shortId && shortId !== 'None' && shortId !== '') ? shortId : fileId;
    const path = `/d/${id}`;
    Utils.copy(path);
};

// Authentication helpers.
const Auth = {
    async logout() {
        const confirmed = await Modal.confirm('Log out', 'Log out of the current session?');
        if (!confirmed) return;
        
        try {
            const res = await fetch('/api/auth/logout', {
                method: 'POST',
                credentials: 'include' 
            });
            
            if (res.ok) {
                window.location.replace('/login');
            } else {
                Toast.show('Logout failed. Refresh and try again', 'error');
            }
        } catch (e) {
            console.error(e);
            Toast.show('Network error', 'error');
        }
    }
};

// Theme System
const Theme = {
    init() {
        const pref = localStorage.getItem('tgstate_theme_pref') || 'auto';
        this.apply(pref);
    },
    
    apply(mode) {
        let theme = mode;
        if (mode === 'auto') {
            theme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
        }
        document.documentElement.setAttribute('data-theme', theme);
        document.documentElement.classList.toggle('dark', theme === 'dark');
        
        // Update Toggle Button Text/Icon if needed (optional)
        const label = document.querySelector('.theme-label');
        if (label) {
            label.textContent = mode === 'auto' ? 'Auto theme' : (mode === 'dark' ? 'Dark theme' : 'Light theme');
        }
    },
    
    cycle() {
        const current = localStorage.getItem('tgstate_theme_pref') || 'auto';
        const next = current === 'auto' ? 'light' : (current === 'light' ? 'dark' : 'auto');
        localStorage.setItem('tgstate_theme_pref', next);
        this.apply(next);
        
        const modeNames = { 'auto': 'Auto theme', 'light': 'Light theme', 'dark': 'Dark theme' };
        Toast.show(`Switched to ${modeNames[next]}`);
    }
};

// Initialize shared UI behavior.
document.addEventListener('DOMContentLoaded', () => {
    Theme.init();

    // Bind theme switchers.
    document.querySelectorAll('.theme-toggle-btn').forEach(btn => {
        btn.addEventListener('click', (e) => {
            e.preventDefault();
            Theme.cycle();
        });
    });

    const toggleBtn = document.querySelector('.menu-toggle');
    const sidebar = document.querySelector('.sidebar');
    const overlay = document.querySelector('.sidebar-overlay');
    
    if (toggleBtn && sidebar) {
        toggleBtn.addEventListener('click', () => {
            sidebar.classList.toggle('active');
            if (overlay) overlay.classList.toggle('active');
        });
    }
    
    if (overlay) {
        overlay.addEventListener('click', () => {
            sidebar.classList.remove('active');
            overlay.classList.remove('active');
        });
    }
});

// Expose to window for inline onclick handlers
window.Theme = Theme;
window.Auth = Auth;
window.Utils = Utils;
window.Modal = Modal;
window.Toast = Toast;
