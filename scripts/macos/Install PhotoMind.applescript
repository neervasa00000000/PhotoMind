-- Self-contained installer. Payload lives in Contents/Resources/
-- (survives macOS App Translocation when run from a DMG).
on run
	try
		set appPath to POSIX path of (path to me)
		set helper to appPath & "Contents/Resources/install-photomind.sh"
		do shell script "/bin/bash " & quoted form of helper
	on error errMsg number errNum
		display alert "PhotoMind install failed" message (errMsg & return & return & "Try System Settings → Privacy & Security → Open Anyway" & return & "(" & errNum & ")") as critical
	end try
end run
