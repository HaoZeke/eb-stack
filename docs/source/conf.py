# -- Project information -----------------------------------------------------
project = "eb-stack"
copyright = "2026, Rohit Goswami"
author = "Rohit Goswami"

# -- General configuration ---------------------------------------------------
extensions = [
    "sphinx_sitemap",
]

templates_path = ["_templates"]
exclude_patterns = []

# -- Options for HTML output -------------------------------------------------
html_theme = "shibuya"
html_static_path = ["_static"]

html_context = {
    "source_type": "github",
    "source_user": "HaoZeke",
    "source_repo": "eb-stack",
    "source_version": "main",
    "source_docs_path": "/docs/source/",
}

html_theme_options = {
    "github_url": "https://github.com/HaoZeke/eb-stack",
    "accent_color": "teal",
    "dark_code": True,
    "globaltoc_expand_depth": 1,
}

html_sidebars = {
    "**": [
        "sidebars/localtoc.html",
        "sidebars/repo-stats.html",
        "sidebars/edit-this-page.html",
    ],
}

html_baseurl = "eb-stack.rgoswami.me"
