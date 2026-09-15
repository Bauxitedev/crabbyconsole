---
icon: lucide/satellite-dish
---

# Running commands remotely

You can connect to the console remotely, to run commands from outside the game. This is useful for running commands if the game is running on a device that doesn't have a physical keyboard (e.g. a gaming handheld).

To set this up, run this in the console:
```
:cons remote start 127.0.0.1 12345
```

The Remote Console server is now active. You can use [ncat](https://nmap.org/ncat/) to connect to it: install it and then run this in a terminal:
```bash
ncat 127.0.0.1 12345 # (1)!
``` 

1. Or `nc 127.0.0.1 12345` on Linux.

By default the server binds to `127.0.0.1`, which means other devices can't connect to it.

To expose the server to other devices, run:
```
:cons remote start 0.0.0.0
```

If the port argument is missing, it will bind to a random available port.

???+ warning "Security warning"
    Binding to `0.0.0.0`  will allow anyone on your local network to connect to your machine and run arbitrary commands on it, so be careful. Only do this on trusted networks.
    
    The protocol does not have authentication at the moment; in the future the remote console may use `ssh` to mitigate this.
    
    See [Security](../40_misc/19_security.md) section for more information.

You can stop the server with `:cons remote stop`. Make sure to stop the server when you don't need it anymore, for security reasons (see [Security](../40_misc/19_security.md) section).

## Connecting from WSL

If you game is running on Windows, and you want to connect to it from WSL, you need to take extra care. [You can't just connect to `localhost`, it won't work](https://learn.microsoft.com/en-us/windows/wsl/networking#accessing-windows-networking-apps-from-linux-host-ip). Instead, do this:

1. In WSL, run this to see the IP address of the Windows host. Write it down somewhere.
```bash
ip route show | grep -i default | awk '{ print $3}'
```
2. Start the console and bind the remote console to `0.0.0.0` by running this in CrabbyConsole:
```
:cons remote start 0.0.0.0 12345
```
3. Now connect to it from WSL by running this command, and replacing `<WINDOWS_IP>` by the IP you found in step 1:
```
nc <WINDOWS_IP> 12345
```

Now it should connect. If not, ensure you allow your game to connect through the Windows Firewall.

???+ warning "Security warning"
    As mentioned before, binding to `0.0.0.0` does have security implications, because anyone on your local network can connect and run arbitrary commands on your computer. To mitigate this, you can enable [mirrored mode networking](https://learn.microsoft.com/en-us/windows/wsl/networking#mirrored-mode-networking) in WSL. This allows you to bind the remote console to `127.0.0.1` instead, and just connect to `localhost` from within WSL. This is more secure, but also more effort to setup.

