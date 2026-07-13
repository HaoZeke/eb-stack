# -- Project information -----------------------------------------------------
project = "eb-stack"
copyright = "2026, Rohit Goswami"
author = "Rohit Goswami"
release = "0.2.0"

# -- General configuration ---------------------------------------------------
extensions = [
    "sphinx_sitemap",
    "sphinx_copybutton",
    "sphinx_design",
]

templates_path = ["_templates"]
exclude_patterns = []

# -- Options for HTML output -------------------------------------------------
html_theme = "shibuya"
html_static_path = ["_static"]
html_logo = "_static/logo.svg"
html_favicon = "_static/favicon.svg"
html_title = "eb-stack documentation"
html_baseurl = "https://eb-stack.rgoswami.me/"
html_css_files = ["custom.css"]

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
    "nav_links": [
        {
            "title": "Tutorial",
            "url": "tutorial",
        },
        {
            "title": "Annual bump",
            "url": "howto/run-annual-bump",
        },
        {
            "title": "CLI",
            "url": "reference/cli",
        },
        {
            "title": "GitHub",
            "url": "https://github.com/HaoZeke/eb-stack",
            "external": True,
        },
    ],
}

html_sidebars = {
    "**": [
        "sidebars/localtoc.html",
        "sidebars/repo-stats.html",
        "sidebars/edit-this-page.html",
    ],
}

# sphinx-sitemap
sitemap_url_scheme = "{link}"
