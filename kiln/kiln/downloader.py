"""Streaming HTTP download into the downloads folder.

Download servers drop connections now and then (especially for free accounts,
which get throttled), so a dropped or stalled connection is retried, carrying
on from where it stopped, and other mirrors are tried before giving up.
"""
from __future__ import annotations

import re
import time
from pathlib import Path
from typing import Callable, Iterable
from urllib.parse import unquote, urlparse

import requests

from .nexus import session

ATTEMPTS = 6
RETRY_WAIT = (2, 5, 10, 20, 30)        # seconds between attempts


class DownloadError(RuntimeError):
    pass


class _Retry(Exception):
    """A problem worth another attempt."""


def _safe(name: str) -> str:
    name = Path(unquote(name)).name
    return re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", name).strip(" .") or "download.bin"


def describe(e: BaseException) -> str:
    """A short, readable reason for a failed request."""
    s = str(e)
    host = re.search(r"host='([^']+)'", s)
    where = f" ({host.group(1)})" if host else ""
    if isinstance(e, requests.exceptions.ReadTimeout) or "Read timed out" in s:
        return "the download server stopped sending data" + where
    if isinstance(e, requests.exceptions.ConnectTimeout) or "timed out" in s:
        return "the download server didn't answer in time" + where
    if "NameResolution" in s or "Name or service not known" in s or "getaddrinfo" in s:
        return "no internet connection (couldn't look up the server)" + where
    if "Connection reset" in s or "RemoteDisconnected" in s or "Connection aborted" in s \
            or isinstance(e, requests.exceptions.ChunkedEncodingError):
        return "the download server dropped the connection" + where
    if "SSL" in s:
        return "a secure connection to the server failed" + where
    if isinstance(e, requests.exceptions.ConnectionError):
        return "couldn't connect to the download server" + where
    return s.splitlines()[0] if s else type(e).__name__


def download(urls: str | Iterable[str] | Callable[[], list[str]], dest_dir: Path,
             progress: Callable[[int, int], None] | None = None,
             cancelled: Callable[[], bool] | None = None, fallback_name: str = "",
             attempts: int = ATTEMPTS, sleep: Callable[[float], None] = time.sleep) -> Path:
    """Download into dest_dir and return the file.

    urls: one URL, a list of mirrors, or a function returning fresh ones (called
    again when a link has expired)."""
    dest_dir.mkdir(parents=True, exist_ok=True)
    refresh = urls if callable(urls) else None
    mirrors = list(refresh()) if refresh else ([urls] if isinstance(urls, str) else list(urls))
    if not mirrors:
        raise DownloadError("No download link.")
    state = {"out": None, "part": None, "total": 0}
    last = ""
    for attempt in range(attempts):
        if cancelled and cancelled():
            break
        url = mirrors[attempt % len(mirrors)]
        try:
            return _attempt(url, dest_dir, state, progress, cancelled, fallback_name)
        except _Retry as e:
            last = str(e)
            if "expired" in last and refresh:
                try:
                    mirrors = list(refresh()) or mirrors
                except Exception as re_:  # noqa: BLE001
                    raise DownloadError(f"{last}, and a new link couldn't be fetched: {re_}") from None
        except DownloadError:
            _drop(state)
            raise
        except (requests.exceptions.ConnectionError, requests.exceptions.Timeout,
                requests.exceptions.ChunkedEncodingError, OSError) as e:
            if isinstance(e, OSError) and not isinstance(e, requests.exceptions.RequestException) \
                    and e.errno not in (None, 104, 110, 111, 113):      # disk errors: don't retry
                _drop(state)
                raise
            last = describe(e)
        if attempt + 1 < attempts:
            sleep(RETRY_WAIT[min(attempt, len(RETRY_WAIT) - 1)])
    if cancelled and cancelled():
        _drop(state)
        raise DownloadError("Cancelled")
    _drop(state)
    raise DownloadError(f"{last[:1].upper()}{last[1:]}. Gave up after {attempts} tries.")


def _drop(state):
    if state["part"]:
        state["part"].unlink(missing_ok=True)


def _attempt(url, dest_dir, state, progress, cancelled, fallback_name) -> Path:
    part: Path | None = state["part"]
    have = part.stat().st_size if part and part.exists() else 0
    headers = {"Range": f"bytes={have}-"} if have else {}
    with session.get(url, stream=True, timeout=(20, 60), headers=headers) as r:
        if r.status_code in (401, 403, 410):
            raise _Retry(f"the download link expired (HTTP {r.status_code})")
        if r.status_code in (408, 429) or r.status_code >= 500:
            raise _Retry(f"the download server is busy (HTTP {r.status_code})")
        if r.status_code == 416 and have and have == state["total"]:
            part.replace(state["out"])                 # it had already finished
            return state["out"]
        if r.status_code >= 400:
            raise DownloadError(f"Download server returned HTTP {r.status_code}")
        if state["out"] is None:
            name = ""
            m = re.search(r"filename\*?=(?:UTF-8'')?\"?([^\";]+)",
                          r.headers.get("content-disposition", ""), re.I)
            if m:
                name = m.group(1)
            name = _safe(name or fallback_name or Path(urlparse(r.url).path).name)
            state["out"] = dest_dir / name
            state["part"] = part = state["out"].with_name(name + ".part")
            part.unlink(missing_ok=True)
            have = 0
        resumed = have and r.status_code == 206
        if not resumed:
            have = 0                                    # server sent the whole file again
            state["total"] = int(r.headers.get("content-length") or 0)
        elif not state["total"]:
            m = re.search(r"/(\d+)$", r.headers.get("content-range", ""))
            state["total"] = int(m.group(1)) if m else 0
        total, done = state["total"], have
        with open(part, "ab" if resumed else "wb") as fh:
            for chunk in r.iter_content(256 * 1024):
                if cancelled and cancelled():
                    raise DownloadError("Cancelled")
                fh.write(chunk)
                done += len(chunk)
                if progress:
                    progress(done, total)
        if total and done < total:
            raise _Retry(f"the download stopped early ({done} of {total} bytes)")
        if total and done > total:
            part.unlink(missing_ok=True)
            raise DownloadError(f"Download is larger than expected ({done} of {total} bytes)")
        part.replace(state["out"])
        return state["out"]
