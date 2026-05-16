document.addEventListener('DOMContentLoaded', () => {
    // --- Global Variables ---
    const uploadArea = document.getElementById('upload-zone');
    const fileInput = document.getElementById('file-picker');
    const progressArea = document.getElementById('prog-zone');
    const doneArea = document.getElementById('done-zone');
    const searchInput = document.getElementById('file-search');
    
    // --- Copy Link Delegation ---
    function resolveItemUrl(item) {
        if (!item) return '';

        let url = '';
        const downloadLink = item.querySelector('a[href^="/d/"]');
        if (downloadLink && downloadLink.href) {
            url = downloadLink.href;
        }

        if (!url) {
            const img = item.querySelector('img[src^="/d/"]');
            if (img && img.src) {
                url = img.src;
            }
        }

        if (!url) {
            const dsUrl = item.dataset.fileUrl;
            if (dsUrl && dsUrl !== 'undefined') {
                url = dsUrl.startsWith('/') ? window.location.origin + dsUrl : dsUrl;
            }
        }

        if (!url || url.includes('undefined')) {
            const shortId = item.dataset.shortId;
            const fileId = item.dataset.fileId;
            const id = (shortId && shortId !== 'None' && shortId !== '') ? shortId : fileId;
            url = window.location.origin + `/d/${id}`;
        }

        if (url.includes('undefined')) {
            console.warn('Constructed URL contained undefined, falling back to raw fileId');
            url = window.location.origin + '/d/' + (item.dataset.fileId || 'error');
        }

        return url;
    }

    document.addEventListener('click', (e) => {
        const btn = e.target.closest('.copy-link-btn');
        if (!btn) return;
        
        // Prevent default if it's a link (though it's a button)
        e.preventDefault();
        e.stopPropagation();

        const item = btn.closest('.file-item, .image-card');
        if (!item) return; // Should exist
        
        // Inline handlers keep priority for any special legacy cases.
        if (btn.hasAttribute('onclick')) return;

        const shortId = item.dataset.shortId;
        const fileId = item.dataset.fileId;
        const filename = item.dataset.filename;
        
        const url = resolveItemUrl(item);

        if (window.copyLink) {
             Utils.copy(url);
        } else {
             Utils.copy(url);
        }
    });

    document.addEventListener('click', (e) => {
        const card = e.target.closest('.image-card');
        if (!card) return;
        if (e.target.closest('button, a, input, label')) return;
        Utils.copy(resolveItemUrl(card));
    });

    document.addEventListener('keydown', (e) => {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        const card = e.target.closest('.image-card');
        if (!card || e.target.closest('button, a, input, label')) return;
        e.preventDefault();
        Utils.copy(resolveItemUrl(card));
    });

    // --- Search Functionality ---
    if (searchInput) {
        searchInput.addEventListener('input', (e) => {
            const term = e.target.value.toLowerCase();
            // Select both file list items and image grid cards
            const items = document.querySelectorAll('.file-item, .image-card');
            items.forEach(item => {
                const name = (item.dataset.filename || '').toLowerCase();
                if (name.includes(term)) {
                    item.style.display = ''; // Reset to default (grid or flex)
                } else {
                    item.style.display = 'none';
                }
            });
        });
    }

    // --- Upload Logic ---
    if (uploadArea && fileInput) {
        // Prevent double dialog by stopping propagation from input
        fileInput.addEventListener('click', (e) => e.stopPropagation());

        uploadArea.addEventListener('click', (e) => {
             // Only trigger if not clicking the input itself (though propagation stop handles it, this is extra safety)
             if (e.target !== fileInput) {
                 fileInput.click();
             }
        });

        uploadArea.addEventListener('dragover', (event) => {
            event.preventDefault();
            uploadArea.style.borderColor = 'var(--primary-color)';
            uploadArea.style.backgroundColor = 'var(--bg-surface-hover)';
        });

        uploadArea.addEventListener('dragleave', () => {
            uploadArea.style.borderColor = '';
            uploadArea.style.backgroundColor = '';
        });

        uploadArea.addEventListener('drop', (event) => {
            event.preventDefault();
            uploadArea.style.borderColor = '';
            uploadArea.style.backgroundColor = '';
            const files = event.dataTransfer.files;
            if (files.length > 0) {
                handleFiles(files);
            }
        });

        fileInput.addEventListener('change', ({ target }) => {
            if (target.files.length > 0) {
                handleFiles(target.files);
            }
        });
    }

    // Queue system for uploads
    const uploadQueue = [];
    let isUploading = false;

    function handleFiles(files) {
        if (progressArea) progressArea.innerHTML = ''; 
        
        for (const file of files) {
            uploadQueue.push(file);
        }
        processQueue();
    }

    function processQueue() {
        if (isUploading || uploadQueue.length === 0) return;
        
        isUploading = true;
        const file = uploadQueue.shift();
        uploadFile(file).then(() => {
            isUploading = false;
            processQueue();
        });
    }

    function uploadFile(file) {
        return new Promise((resolve) => {
            const formData = new FormData();
            formData.append('file', file, file.name);
            
            const xhr = new XMLHttpRequest();
            xhr.open('POST', '/api/upload', true);
            const fileId = `temp-${Date.now()}-${Math.random().toString(36).substr(2, 5)}`;
            const safeFileName = escapeHtml(file.name);

            // Initial progress UI.
            const progressHTML = `
                <div class="card" id="progress-${fileId}" style="padding: 16px;">
                    <div style="display: flex; justify-content: space-between; margin-bottom: 8px;">
                        <span style="font-size: 14px; font-weight: 700;">${safeFileName}</span>
                        <span class="percent" style="font-size: 12px; color: var(--text-secondary);">0%</span>
                    </div>
                    <div class="progress-bar">
                        <div class="progress-fill" style="width: 0%;"></div>
                    </div>
                </div>`;
            
            if (progressArea) progressArea.insertAdjacentHTML('beforeend', progressHTML);
            const progressEl = document.querySelector(`#progress-${fileId} .progress-fill`);
            const percentEl = document.querySelector(`#progress-${fileId} .percent`);

            xhr.upload.onprogress = ({ loaded, total }) => {
                const percent = Math.floor((loaded / total) * 100);
                if (progressEl) progressEl.style.width = `${percent}%`;
                if (percentEl) percentEl.textContent = `${percent}%`;
            };

            xhr.onload = () => {
                const progressRow = document.getElementById(`progress-${fileId}`);
                if (progressRow) progressRow.remove();

                if (xhr.status === 200) {
                    const response = JSON.parse(xhr.responseText);
                    const fileUrl = response.url;
                    const safeFileUrl = escapeHtml(fileUrl);
                    const jsFileUrl = escapeJsString(fileUrl);
                    
                    // Success Toast
                    if (window.Toast) Toast.show(`${file.name} uploaded`);
                    
                    // Add to done area
                    const successHTML = `
                        <div class="card" style="padding: 16px; border-left: 4px solid var(--success-color);">
                            <div style="display: flex; justify-content: space-between; align-items: center;">
                                <div style="overflow: hidden; margin-right: 12px;">
                                    <div style="font-size: 14px; font-weight: 700; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">${safeFileName}</div>
                                    <a href="${safeFileUrl}" target="_blank" style="font-size: 12px; color: var(--primary-color);">${safeFileUrl}</a>
                                </div>
                                <button class="btn btn-secondary btn-sm" onclick="Utils.copy('${jsFileUrl}')">Copy</button>
                            </div>
                        </div>`;
                    if (doneArea) doneArea.insertAdjacentHTML('afterbegin', successHTML);
                } else {
                    let errorMsg = "Upload failed";
                    try {
                        const parsed = JSON.parse(xhr.responseText);
                        const detail = parsed && parsed.detail;
                        if (typeof detail === 'string') {
                            errorMsg = detail;
                        } else if (detail && typeof detail === 'object') {
                            errorMsg = detail.message || errorMsg;
                        } else if (parsed && parsed.message) {
                            errorMsg = parsed.message;
                        }
                    } catch (e) {}
                    
                    if (window.Toast) Toast.show(errorMsg, 'error');
                }
                resolve();
            };

            xhr.onerror = () => {
                const progressRow = document.getElementById(`progress-${fileId}`);
                if (progressRow) progressRow.remove();
                if (window.Toast) Toast.show('Network error', 'error');
                resolve();
            };

            xhr.send(formData);
        });
    }

    // --- Batch Actions ---
    const selectAllCheckbox = document.getElementById('select-all-checkbox');
    const batchDeleteBtn = document.getElementById('batch-delete-btn');
    const copyLinksBtn = document.getElementById('copy-links-btn');
    const selectionCounter = document.getElementById('selection-counter');
    const batchActionsBar = document.getElementById('batch-actions-bar');
    const formatOptions = document.querySelectorAll('.format-option');

    function updateBatchControls() {
        const checkboxes = document.querySelectorAll('.file-checkbox');
        const checked = document.querySelectorAll('.file-checkbox:checked');
        const count = checked.length;
        
        if (selectionCounter) selectionCounter.textContent = count > 0 ? `${count} selected` : '0 selected';
        
        if (batchActionsBar) {
            if (count > 0) {
                batchActionsBar.classList.remove('hidden');
            } else {
                batchActionsBar.classList.add('hidden');
            }
        }

        if (selectAllCheckbox) selectAllCheckbox.checked = (count > 0 && count === checkboxes.length);
    }

    if (selectAllCheckbox) {
        selectAllCheckbox.addEventListener('change', (e) => {
            document.querySelectorAll('.file-checkbox').forEach(cb => {
                cb.checked = e.target.checked;
            });
            updateBatchControls();
        });
    }

    // Delegation for dynamic checkboxes
    document.addEventListener('change', (e) => {
        if (e.target.classList.contains('file-checkbox')) {
            updateBatchControls();
        }
    });

    // Format selection (Image Hosting)
    if (formatOptions) {
        formatOptions.forEach(opt => {
            opt.addEventListener('click', () => {
                formatOptions.forEach(o => o.classList.remove('active'));
                opt.classList.add('active');
            });
        });
    }

    // Batch Copy
    if (copyLinksBtn) {
        copyLinksBtn.addEventListener('click', () => {
            const checked = document.querySelectorAll('.file-checkbox:checked');
            if (checked.length === 0) return;

            const activeFormatBtn = document.querySelector('.format-option.active');
            const format = activeFormatBtn ? activeFormatBtn.dataset.format : 'url';
            
            const links = Array.from(checked).map(cb => {
                const item = cb.closest('.file-item, .image-card');
                let url = '';

                // 1. Download link.
                const downloadLink = item.querySelector('a[href^="/d/"]');
                if (downloadLink && downloadLink.href) {
                    url = downloadLink.href;
                }
                
                // 2. Image src.
                if (!url) {
                    const img = item.querySelector('img[src^="/d/"]');
                    if (img && img.src) {
                        url = img.src;
                    }
                }
                
                // 3. Fallback: Dataset
                if (!url) {
                    const dsUrl = item.dataset.fileUrl;
                    if (dsUrl && dsUrl !== 'undefined') {
                        url = dsUrl;
                        if (url.startsWith('/')) {
                            url = window.location.origin + url;
                        }
                    }
                }
                
                // 4. Final Fallback
                if (!url || url.includes('undefined')) {
                    const shortId = item.dataset.shortId;
                    const fileId = item.dataset.fileId;
                    const id = (shortId && shortId !== 'None' && shortId !== '') ? shortId : fileId;
                    url = window.location.origin + `/d/${id}`;
                }
                
                const name = item.dataset.filename;

                if (format === 'markdown') return `![${name}](${url})`;
                if (format === 'html') return `<img src="${url}" alt="${name}">`;
                return url;
            });

            Utils.copy(links.join('\n'));
        });
    }

    // Batch Delete
    if (batchDeleteBtn) {
        batchDeleteBtn.addEventListener('click', async () => {
            const checked = document.querySelectorAll('.file-checkbox:checked');
            if (checked.length === 0) return;

            const confirmed = await Modal.confirm('Batch delete', `Delete ${checked.length} selected files?`);
            if (!confirmed) return;

            const fileIds = Array.from(checked).map(cb => cb.dataset.fileId);
            
            fetch('/api/batch_delete', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ file_ids: fileIds })
            })
            .then(res => res.json())
            .then(data => {
                if (data.deleted) {
                    data.deleted.forEach(item => {
                         const id = item.details?.file_id || item; 
                         removeFileElement(id);
                    });
                    if (window.Toast) Toast.show(`Deleted ${data.deleted.length} files`);
                }
                updateBatchControls();
            });
        });
    }

    // --- SSE & Realtime Updates ---
    const fileListContainer = document.getElementById('file-list-disk');
    if (fileListContainer) {
        let eventSource = null;

        const connectSSE = () => {
            if (eventSource) {
                eventSource.close();
            }
            eventSource = new EventSource('/api/file-updates');

            eventSource.onmessage = (event) => {
                const msg = JSON.parse(event.data);
                const action = msg && msg.action ? msg.action : 'add';
                if (action === 'delete') {
                    removeFileElement(msg.file_id);
                    updateBatchControls();
                    return;
                }
                addNewFileElement(msg);
            };

            eventSource.onerror = () => {
                try { eventSource.close(); } catch (_) {}
                setTimeout(connectSSE, 5000);
            };
        };

        connectSSE();
    }

    function formatDateValue(value) {
        if (!value) return '';
        const d = new Date(value);
        if (!isNaN(d.getTime())) return d.toISOString().split('T')[0];
        const s = String(value);
        return s.split(' ')[0].split('T')[0];
    }

    function escapeHtml(value) {
        return String(value ?? '').replace(/[&<>"']/g, (ch) => ({
            '&': '&amp;',
            '<': '&lt;',
            '>': '&gt;',
            '"': '&quot;',
            "'": '&#39;'
        })[ch]);
    }

    function escapeJsString(value) {
        return String(value ?? '').replace(/\\/g, '\\\\').replace(/'/g, "\\'");
    }

    function addNewFileElement(file) {
        const isGridView = document.querySelector('.image-grid') !== null;
        const container = document.getElementById('file-list-disk');
        
        // Remove empty state if exists
        const emptyState = container.querySelector('div[style*="text-align: center"]');
        if (emptyState) emptyState.remove();

        const formattedSize = (file.filesize / (1024 * 1024)).toFixed(2) + " MB";
        const formattedDate = formatDateValue(file.upload_date);
        const safeId = file.file_id.replace(':', '-');
        const safeFileId = escapeHtml(file.file_id);
        const safeShortId = escapeHtml(file.short_id || '');
        const safeFilename = escapeHtml(file.filename);
        
        // URL construction: always use /d/{id}, preferring short IDs.
        let fileUrl = `/d/${file.short_id || file.file_id}`;
        const safeFileUrl = escapeHtml(fileUrl);

        let html = '';
        if (isGridView) {
             html = `
                <div class="file-item image-card" id="file-item-${safeId}" data-file-id="${safeFileId}" data-file-url="${safeFileUrl}" data-filename="${safeFilename}" data-short-id="${safeShortId}" role="button" tabindex="0" title="Copy direct image link">
                    <div class="image-thumb">
                        <img src="${safeFileUrl}" loading="lazy" alt="${safeFilename}">
                        <div class="image-check">
                            <input type="checkbox" class="file-checkbox" data-file-id="${safeFileId}" aria-label="Select ${safeFilename}">
                        </div>
                    </div>
                    <div class="image-info">
                        <span class="file-title" title="${safeFilename}">${safeFilename}</span>
                        <span class="file-subtitle">${formattedSize} · ${formattedDate}</span>
                        <div class="image-actions">
                            <button class="btn btn-secondary btn-sm copy-link-btn">Copy</button>
                            <button class="btn btn-secondary btn-sm delete" style="color: var(--danger-color);" onclick="deleteFile('${safeFileId}')" aria-label="Delete ${safeFilename}">
                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18"></path><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"></path><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path></svg>
                            </button>
                        </div>
                    </div>
                </div>`;
        } else {
            html = `
                <tr class="file-item" id="file-item-${safeId}" data-file-id="${safeFileId}" data-file-url="${safeFileUrl}" data-filename="${safeFilename}" data-short-id="${safeShortId}">
                    <td><input type="checkbox" class="file-checkbox" data-file-id="${safeFileId}" aria-label="Select ${safeFilename}"></td>
                    <td>
                        <div class="file-name-cell">
                            <span class="file-icon">
                                <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><path d="M14 2v6h6"></path></svg>
                            </span>
                            <span style="min-width: 0;">
                                <span class="file-title">${safeFilename}</span>
                                <span class="file-subtitle">${safeFileUrl}</span>
                            </span>
                        </div>
                    </td>
                    <td class="text-muted">${formattedSize}</td>
                    <td class="text-muted">${formattedDate}</td>
                    <td style="text-align: right;">
                        <div class="row-actions">
                            <a href="${safeFileUrl}" class="btn btn-ghost" title="Download" aria-label="Download ${safeFilename}">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><path d="m7 10 5 5 5-5"></path><path d="M12 15V3"></path></svg>
                            </a>
                            <button class="btn btn-ghost copy-link-btn" title="Copy link" aria-label="Copy ${safeFilename} link">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="1"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
                            </button>
                            <button class="btn btn-ghost delete" style="color: var(--danger-color);" onclick="deleteFile('${safeFileId}')" title="Delete" aria-label="Delete ${safeFilename}">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18"></path><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"></path><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path></svg>
                            </button>
                        </div>
                    </td>
                </tr>`;
        }

        container.insertAdjacentHTML('afterbegin', html);
    }

    // --- Global Helpers ---
    window.deleteFile = async (fileId) => {
        const confirmed = await Modal.confirm('Delete file', 'Delete this file?');
        if (!confirmed) return;
        fetch(`/api/files/${fileId}`, { method: 'DELETE' })
            .then(async (res) => {
                let data = null;
                try { data = await res.json(); } catch (e) {}
                return { ok: res.ok, data };
            })
            .then(({ ok, data }) => {
                if (ok && data && data.status === 'ok') {
                    removeFileElement(fileId);
                    if (window.Toast) Toast.show('File deleted');
                    updateBatchControls();
                } else {
                    const msg = data?.detail?.message || data?.message || 'Delete failed';
                    if (window.Toast) Toast.show(msg, 'error');
                }
            });
    };

    function removeFileElement(fileId) {
        const el = document.getElementById(`file-item-${fileId.replace(':', '-')}`);
        if (el) el.remove();
        
        // Check if empty
        const container = document.getElementById('file-list-disk');
        if (container && container.children.length === 0) {
            // Re-render empty state logic if needed, or let user refresh
            // Simple text fallback
            const isGridView = document.querySelector('.image-grid') !== null;
            if (isGridView) {
                 container.innerHTML = `
                    <div style="grid-column: 1/-1; padding: 40px; text-align: center; color: var(--text-tertiary);">
                        <p>No images</p>
                    </div>`;
            } else {
                 container.innerHTML = `
                    <tr>
                        <td colspan="5" style="padding: 48px; text-align: center;">
                            <div class="text-muted">No files</div>
                        </td>
                    </tr>`;
            }
        }
    }
});
