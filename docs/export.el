;;; export.el --- Publish docs/orgmode to docs/source -*- lexical-binding: t; -*-

;; Org 9.7 stores the text of ~code~ and =verbatim= as an
;; org-element-deferred object.  ox-rst hands that object to
;; replace-regexp-in-string, which signals
;; wrong-type-argument stringp.  Whether org-element-property resolves
;; the object depends on the Org that byte-compiled ox-rst: a standard
;; property is inlined as org-element-property-raw and stays deferred.
;;
;; ox-rst is GPL-2+ and this repository is MIT, so the file is not
;; stored in the tree.  Fetch the pinned upstream commit into a cache
;; outside the tree ($XDG_CACHE_HOME/eb-stack/ox-rst/<commit>/, or
;; EB_STACK_OX_RST_CACHE), check the sha256 below, then load that
;; source.  Resolve deferred values on the parse tree before any
;; transcoder runs.

(require 'org)
(require 'ox-publish)
(require 'ob-dot)

(defconst eb-stack-ox-rst-revision
  "b73eff187eebac24b457688bfd27f09eff434860")
(defconst eb-stack-ox-rst-sha256
  "17ac300861f22e62166785a8ac1b97d32bb8a131ee5ba1b7a9db8977b07e7ede")
(defconst eb-stack-ox-rst-url
  (concat "https://raw.githubusercontent.com/msnoigrs/ox-rst/"
          eb-stack-ox-rst-revision
          "/ox-rst.el"))

(defvar eb-stack-export-failures nil
  "Alist of (file . message) for pages that failed to publish.")

(defun eb-stack-export-fail (format-string &rest args)
  "Print FORMAT-STRING with ARGS and exit non-zero."
  (apply #'message format-string args)
  (kill-emacs 1))

(defun eb-stack-ox-rst-cache-file ()
  "Absolute path of the pinned ox-rst source, outside this repository."
  (expand-file-name
   (concat eb-stack-ox-rst-revision "/ox-rst.el")
   (or (getenv "EB_STACK_OX_RST_CACHE")
       (expand-file-name
        "eb-stack/ox-rst"
        (or (getenv "XDG_CACHE_HOME")
            (expand-file-name ".cache"
                              (or (getenv "HOME")
                                  temporary-file-directory)))))))

(defun eb-stack-file-sha256 (file)
  "Return the sha256 hex digest of FILE."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally file)
    (secure-hash 'sha256 (current-buffer))))

(defun eb-stack-check-ox-rst-sha256 (file)
  "Exit non-zero when FILE is not the pinned ox-rst digest."
  (let ((got (eb-stack-file-sha256 file)))
    (unless (string= got eb-stack-ox-rst-sha256)
      (when (file-exists-p file)
        (delete-file file))
      (eb-stack-export-fail
       "eb-stack export failed: ox-rst %s checksum mismatch (expected %s, got %s) from %s"
       eb-stack-ox-rst-revision eb-stack-ox-rst-sha256 got eb-stack-ox-rst-url))))

(defun eb-stack-fetch-ox-rst (dest)
  "Download the pinned ox-rst to DEST, or exit non-zero."
  (let ((part (concat dest ".partial")))
    (make-directory (file-name-directory dest) t)
    (when (file-exists-p part)
      (delete-file part))
    (message "eb-stack export: fetching ox-rst %s" eb-stack-ox-rst-revision)
    (condition-case err
        (let ((status
               (with-temp-buffer
                 (call-process
                  "curl" nil (list (current-buffer) nil) nil
                  "-fsSL" "--retry" "3" "--retry-delay" "1"
                  "-o" part eb-stack-ox-rst-url))))
          (unless (eq status 0)
            (when (file-exists-p part)
              (delete-file part))
            (eb-stack-export-fail
             "eb-stack export failed: could not fetch ox-rst %s from %s (curl exited %s)"
             eb-stack-ox-rst-revision eb-stack-ox-rst-url status)))
      (error
       (when (file-exists-p part)
         (delete-file part))
       (eb-stack-export-fail
        "eb-stack export failed: could not fetch ox-rst %s from %s (%s)"
        eb-stack-ox-rst-revision eb-stack-ox-rst-url
        (error-message-string err))))
    (unless (file-readable-p part)
      (eb-stack-export-fail
       "eb-stack export failed: fetch of ox-rst %s from %s wrote no file"
       eb-stack-ox-rst-revision eb-stack-ox-rst-url))
    (eb-stack-check-ox-rst-sha256 part)
    (rename-file part dest t)))

(defun eb-stack-load-pinned-ox-rst ()
  "Verify the pinned ox-rst and load it.  Fetch when the cache misses."
  (let ((dest (eb-stack-ox-rst-cache-file)))
    (unless (and (file-readable-p dest)
                 (string= (eb-stack-file-sha256 dest) eb-stack-ox-rst-sha256))
      (when (file-exists-p dest)
        (delete-file dest))
      (eb-stack-fetch-ox-rst dest))
    (eb-stack-check-ox-rst-sha256 dest)
    ;; NOSUFFIX so a previously byte-compiled ox-rst.elc is not loaded.
    (load dest nil nil t)))

(eb-stack-load-pinned-ox-rst)

(message "eb-stack export: Org %s; ox-rst %s"
         (org-version) eb-stack-ox-rst-revision)

(setq org-confirm-babel-evaluate nil)

(defun eb-stack-resolve-deferred (tree _backend _info)
  "Replace deferred properties in TREE with their computed values.
Org 9.7 leaves :value of code and verbatim objects deferred.
ox-rst expects a string.  Resolving here makes the export independent
of whether org-element-property was inlined to the raw accessor."
  (when (fboundp 'org-element-properties-resolve)
    (org-element-map tree t
      (lambda (node)
        (unless (stringp node)
          (org-element-properties-resolve node t)))))
  tree)

(add-to-list 'org-export-filter-parse-tree-functions
             #'eb-stack-resolve-deferred)

;; Sphinx resolves :doc: roles to rendered pages.  ox-rst otherwise
;; exports file links between Org sources as literal links to
;; generated .rst files.
(defun eb-stack-rst-doc-link-filter (text backend _info)
  (if (org-export-derived-backend-p backend 'rst)
      (replace-regexp-in-string
       "`\\([^`]+\\) <\\([^>]+\\)\\.rst>`_"
       ":doc:`\\1 <\\2>`"
       text)
    text))

(add-to-list 'org-export-filter-link-functions
             #'eb-stack-rst-doc-link-filter)

(setq org-publish-project-alist
      '(("sphinx-rst"
         :base-directory "./orgmode/"
         :base-extension "org"
         :publishing-directory "./source/"
         :publishing-function org-rst-publish-to-rst
         :recursive t
         :headline-levels 4)
        ("sphinx-images"
         :base-directory "./orgmode/"
         :base-extension "svg\\|png\\|jpg\\|jpeg\\|webp"
         :publishing-directory "./source/"
         :publishing-function org-publish-attachment
         :recursive t)
        ("sphinx" :components ("sphinx-rst" "sphinx-images"))))

(defun eb-stack-publish-file (orig filename &optional project no-cache)
  "Call ORIG to publish FILENAME, recording a failure instead of stopping."
  (condition-case err
      (funcall orig filename project no-cache)
    (error
     (let ((msg (error-message-string err)))
       (message "eb-stack export failed: %s: %s" filename msg)
       (push (cons filename msg) eb-stack-export-failures)
       nil))))

(advice-add 'org-publish-file :around #'eb-stack-publish-file)

(setq eb-stack-export-failures nil)
(org-publish "sphinx" t)

(when eb-stack-export-failures
  (message "eb-stack export: %d page(s) failed" (length eb-stack-export-failures))
  (dolist (fail (nreverse eb-stack-export-failures))
    (message "eb-stack export failed: %s: %s" (car fail) (cdr fail)))
  (kill-emacs 1))
