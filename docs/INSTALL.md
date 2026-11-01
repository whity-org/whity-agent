# Installing Whity Agent

[العربية](#تثبيت-وكيل-whity)

Download the file for your computer from the [latest release](https://github.com/whity-org/whity-agent/releases/latest).

| Your computer | File |
|---|---|
| Windows | `Whity Agent_x.y.z_x64-setup.exe`, or `arm64-setup.exe` for an Arm PC. The `.msi` is for IT departments. |
| macOS | `Whity Agent_x.y.z_universal.dmg` |
| Linux | `.AppImage`, `.deb` or `.rpm` |

## Why your computer warns you

Whity Agent is free and open source, and its installers aren't signed with a paid certificate yet. So Windows and macOS warn you the first time. Updates after that are signed by us, and the app checks each one before installing it.

### Windows

If you see "Windows protected your PC":

1. Select **More info**.
2. Check that the file name is the one you downloaded.
3. Select **Run anyway**.

### macOS

If macOS says the app "can't be opened" or "cannot be verified":

1. Open the disk image and drag Whity Agent to Applications.
2. Try to open it once, then close the message.
3. Open **System Settings**, then **Privacy & Security**.
4. Scroll down and select **Open Anyway** next to Whity Agent, then confirm.

### Linux

For an AppImage, allow it to run first, then open it:

```bash
chmod +x Whity*.AppImage
```

Your sign-in is kept in the Secret Service (GNOME Keyring or KWallet). Without one, Whity Agent still works, but you sign in again each time you open it.

## Check the file you downloaded

Each release has a `SHA256SUMS.txt`. The checksum of your file should match its line there.

Windows, in PowerShell:

```powershell
Get-FileHash "Whity Agent_x.y.z_x64-setup.exe" -Algorithm SHA256
```

macOS:

```bash
shasum -a 256 "Whity Agent_x.y.z_universal.dmg"
```

Linux:

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
```

## Updates

Whity Agent looks for updates when it opens and once a day, and downloads them in the background. It shows you when one is ready, and restarts only when you choose. An update without our signature is never installed.

---

<div dir="rtl">

# تثبيت وكيل Whity

نزّل الملف المناسب لجهازك من [أحدث إصدار](https://github.com/whity-org/whity-agent/releases/latest).

| جهازك | الملف |
|---|---|
| Windows | ‏`Whity Agent_x.y.z_x64-setup.exe`، أو `arm64-setup.exe` لأجهزة Arm. ملف `.msi` مخصص لأقسام تقنية المعلومات. |
| macOS | ‏`Whity Agent_x.y.z_universal.dmg` |
| Linux | ‏`.AppImage` أو `.deb` أو `.rpm` |

## لماذا يحذّرك جهازك

وكيل Whity مجاني ومفتوح المصدر، ولم تُوقَّع ملفات تثبيته بشهادة مدفوعة بعد. لذلك يحذّرك Windows وmacOS في المرة الأولى. أما التحديثات بعد ذلك فموقّعة منا، ويتحقق التطبيق من كل تحديث قبل تثبيته.

### Windows

إذا ظهرت رسالة "Windows protected your PC":

1. اختر **More info** (مزيد من المعلومات).
2. تأكد أن اسم الملف هو الذي نزّلته.
3. اختر **Run anyway** (التشغيل على أي حال).

### macOS

إذا ظهرت رسالة بأن التطبيق "can't be opened" أو "cannot be verified":

1. افتح ملف القرص واسحب وكيل Whity إلى Applications.
2. حاول فتحه مرة واحدة، ثم أغلق الرسالة.
3. افتح **System Settings** (إعدادات النظام)، ثم **Privacy & Security** (الخصوصية والأمان).
4. مرّر إلى الأسفل واختر **Open Anyway** (الفتح على أي حال) بجانب وكيل Whity، ثم أكّد.

### Linux

لملف AppImage، اسمح بتشغيله أولًا، ثم افتحه:

<div dir="ltr">

```bash
chmod +x Whity*.AppImage
```

</div>

يُحفظ تسجيل دخولك في Secret Service ‏(GNOME Keyring أو KWallet). من دونه يعمل وكيل Whity، لكنك تسجّل الدخول من جديد في كل مرة تفتحه.

## تحقق من الملف الذي نزّلته

يتضمن كل إصدار ملف `SHA256SUMS.txt`. يجب أن يطابق المجموع الاختباري لملفك السطر الخاص به هناك.

Windows، في PowerShell:

<div dir="ltr">

```powershell
Get-FileHash "Whity Agent_x.y.z_x64-setup.exe" -Algorithm SHA256
```

</div>

macOS:

<div dir="ltr">

```bash
shasum -a 256 "Whity Agent_x.y.z_universal.dmg"
```

</div>

Linux:

<div dir="ltr">

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
```

</div>

## التحديثات

يبحث وكيل Whity عن التحديثات عند فتحه ومرة كل يوم، وينزّلها في الخلفية. ويخبرك عندما يصبح التحديث جاهزًا، ولا يعيد التشغيل إلا حين تختار ذلك. ولا يُثبَّت أي تحديث لا يحمل توقيعنا.

</div>
