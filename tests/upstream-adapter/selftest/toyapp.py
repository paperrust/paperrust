"""Toy settings for the adapter self-test (views/urls: toyurls.py)."""

from __future__ import annotations


def settings_dict(db_path: str) -> dict:
    return dict(
        DEBUG=False,
        SECRET_KEY="adapter-selftest-only",
        ALLOWED_HOSTS=["testserver", "127.0.0.1", "localhost"],
        ROOT_URLCONF="toyurls",
        INSTALLED_APPS=["django.contrib.contenttypes", "django.contrib.auth", "django.contrib.sessions",
                        "rest_framework", "rest_framework.authtoken"],
        MIDDLEWARE=["django.contrib.sessions.middleware.SessionMiddleware",
                    "django.middleware.common.CommonMiddleware",
                    "django.middleware.csrf.CsrfViewMiddleware",
                    "django.contrib.auth.middleware.AuthenticationMiddleware"],
        DATABASES={"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": db_path,
                               "TEST": {"NAME": db_path}}},
        REST_FRAMEWORK={"DEFAULT_AUTHENTICATION_CLASSES": [
            "rest_framework.authentication.TokenAuthentication",
            "rest_framework.authentication.SessionAuthentication"]},
        PASSWORD_HASHERS=["django.contrib.auth.hashers.MD5PasswordHasher"],
        USE_TZ=True,
        DEFAULT_AUTO_FIELD="django.db.models.AutoField",
    )
