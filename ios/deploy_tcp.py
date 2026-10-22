# /// script
# requires-python = ">=3.12,<3.14"
# dependencies = ["pymobiledevice3==11.42.0"]
# ///
"""Install an app on an iPhone over the network: the same Wi-Fi, or a VPN such as Tailscale.

    uv run --script ios/deploy_tcp.py HOST UDID path/to/App.app

It talks to lockdownd on HOST:62078 with this Mac's pairing with the phone, which
usbmuxd keeps under the phone's UDID, uploads the app and installs it over the one
already there, whose data stays. ios/deploy.sh calls it when the phone is away.
"""

import asyncio
import plistlib
import sys
from pathlib import Path

from pymobiledevice3.exceptions import ConnectionTerminatedError, PyMobileDevice3Exception
from pymobiledevice3.lockdown import create_using_tcp
from pymobiledevice3.pair_records import get_usbmux_pairing_record
from pymobiledevice3.services.installation_proxy import InstallationProxyService


async def install(host: str, udid: str, app: Path) -> None:
    bundle_id = plistlib.loads((app / "Info.plist").read_bytes())["CFBundleIdentifier"]
    record = await get_usbmux_pairing_record(udid)
    if not record:
        sys.exit(f"this Mac is not paired with {udid}: connect the phone by cable once and tap Trust")
    try:
        lockdown = await asyncio.wait_for(
            create_using_tcp(host, identifier=udid, pair_record=record, autopair=False), 30
        )
    except (OSError, asyncio.TimeoutError) as e:
        sys.exit(f"no lockdown at {host}:62078 ({e or 'timed out'}): is Tailscale connected on the phone?")
    try:
        if not lockdown.paired:
            sys.exit("the phone refused this Mac's pairing: unlock it once; if that does not help, re-pair by cable")
        print(f"connected to {lockdown.display_name or udid}, iOS {lockdown.product_version}; uploading…", flush=True)
        shown = -1

        def progress(percent: int, *_: object) -> None:
            nonlocal shown
            if percent >= shown + 25 or percent == 100:
                shown = percent
                print(f"  {percent}%", flush=True)

        async with InstallationProxyService(lockdown) as installer:
            await installer.install_from_local(app, handler=progress)
            found = await installer.lookup(
                {"BundleIDs": [bundle_id], "ReturnAttributes": ["CFBundleShortVersionString", "CFBundleVersion"]}
            )
        info = (found or {}).get(bundle_id)
        if not info:
            sys.exit(f"the install ended, but the phone does not list {bundle_id}")
        print(f"on the phone: {bundle_id} {info.get('CFBundleShortVersionString')} ({info.get('CFBundleVersion')})")
    finally:
        await lockdown.close()


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    try:
        asyncio.run(install(sys.argv[1], sys.argv[2], Path(sys.argv[3])))
    except ConnectionTerminatedError:
        sys.exit("the phone closed the connection: the network dropped, or it does not let this Mac in over the network")
    except PyMobileDevice3Exception as e:
        sys.exit(f"the phone said no: {type(e).__name__}: {e}")
