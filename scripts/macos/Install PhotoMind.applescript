-- PhotoMind installer wrapper.
-- First launch: Right-click this app → Open (required by macOS for unsigned apps).
on run
	try
		set appPath to POSIX path of (path to me)
		set base to do shell script "cd " & quoted form of appPath & "/.. && pwd"
		set helper to base & "/install-photomind.sh"
		do shell script "/bin/bash " & quoted form of helper
	on error errMsg number errNum
		display alert "PhotoMind install failed" message (errMsg & return & return & "Tip: Right-click Install PhotoMind → Open" & return & "(" & errNum & ")") as critical
	end try
end run
