;; Setup Package Manager (to fetch ox-rst automatically)
(require 'package)
(add-to-list 'package-archives '("melpa" . "https://melpa.org/packages/") t)
(package-initialize)

;; Ensure ox-rst is present
(unless (package-installed-p 'ox-rst)
  (package-refresh-contents)
  (package-install 'ox-rst))

(require 'ox-rst)
(require 'ox-publish)

;; Enable org-babel evaluation for dot (graphviz) blocks
(require 'ob-dot)
(setq org-confirm-babel-evaluate nil)

;; Sphinx resolves :doc: roles to rendered pages.  ox-rst otherwise exports
;; file links between Org sources as literal links to generated .rst files.
(defun eb-stack-rst-doc-link-filter (text backend _info)
  (if (org-export-derived-backend-p backend 'rst)
      (replace-regexp-in-string
       "`\\([^`]+\\) <\\([^>]+\\)\\.rst>`_"
       ":doc:`\\1 <\\2>`"
       text)
    text))

(add-to-list 'org-export-filter-link-functions
             #'eb-stack-rst-doc-link-filter)

;; Define the Publishing Project
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

;; Run the publish
(org-publish "sphinx" t)
