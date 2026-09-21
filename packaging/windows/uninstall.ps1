# Removes quietmouse installed by install.ps1, keeping your config.
#
#   irm https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/windows/uninstall.ps1 | iex
#
# The removal itself lives in install.ps1, next to the install it undoes.
& ([scriptblock]::Create((Invoke-RestMethod 'https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/windows/install.ps1' -UseBasicParsing))) -Uninstall
