Fork release of codex-switch. Upstream v20261001.5.0 behavior is kept, with these additions:

- **Account sync.** The TUI Sync tab stores saved ChatGPT accounts in named Redis or Upstash groups. Upload and download open the group list: Enter sends the checked groups, and `a` sends every group. Uploading all also deletes remote groups that are not on this computer. Downloading a group replaces that local group. The upload includes `auth.json`.
- **Language.** Settings defaults to English and can switch the interface to Chinese.
- **Subscription expiry.** The Accounts table shows `active_until` from the ChatGPT subscriptions endpoint after a refresh.
- **Auto warmup.** While the TUI is open, an account can be warmed one minute after its 5-hour or 7-day window resets.

## Quick install

macOS / Linux:

```bash
curl -fsSL https://github.com/Listen3/codex-switch/releases/download/v20261114.1.0/install.sh | bash
```

Windows PowerShell:

```powershell
irm https://github.com/Listen3/codex-switch/releases/download/v20261114.1.0/install.ps1 | iex
```
