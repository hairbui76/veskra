// Veskra landing page: language switch (EN / VI) and the newest release's download links.
// Without JavaScript the page is English and the links point at the build that was current when
// the page was published.
(() => {
  "use strict";

  const REPO = "hairbui76/veskra";
  const ASSET_SUFFIX = { "arm64-v8a": "-arm64-v8a.apk", x86_64: "-x86_64.apk", universal: "-universal.apk", sums: "SHA256SUMS.txt" };

  const VI = {
    "skip": "Bỏ qua, tới nội dung",
    "nav.features": "Tính năng",
    "nav.color": "Màu sắc",
    "nav.download": "Tải về",
    "hero.badge": "Bản thử nghiệm alpha · Android 10+",
    "hero.title1": "Vẽ trên tablet.",
    "hero.title2": "Xuất file chuẩn studio.",
    "hero.lede": "Ứng dụng vẽ miễn phí, mã nguồn mở cho tablet Android. Chạy offline, nhường gần trọn màn hình cho bức vẽ, và chuyển tác phẩm sang Photoshop, Illustrator dưới dạng PSD nhiều layer và SVG/PDF — kèm sẵn profile màu trong file.",
    "hero.cta": "Tải APK",
    "hero.other": "Bản khác",
    "hero.fine": "Dành cho đa số tablet và điện thoại (arm64). Miễn phí · Không cần tài khoản · Không quảng cáo · Không xin quyền Internet.",
    "hero.alt": "Màn hình vẽ của Veskra trên tablet Android: bầu trời chuyển màu với mặt trời cam trên nền cỏ xanh, các nút nổi ở góc và thanh dock phía dưới.",
    "hero.sticker": "viết bằng Rust",
    "feat.kicker": "01 — Có gì bên trong",
    "feat.title": "Đủ công cụ cần dùng, không có thứ phải lướt qua.",
    "feat.canvas.t": "Màn hình dành cho bức vẽ",
    "feat.canvas.d": "Các nút nằm gọn ở góc, thanh dock nhỏ giữ brush, smudge, tẩy và hai màu. Mọi thứ khác chỉ mở ra khi cần và tự đóng khi bạn quay lại vẽ.",
    "feat.picker.t": "Bốn cách chọn màu",
    "feat.picker.d": "Vòng màu, thanh trượt HSB / RGB / LAB / CMYK kèm mã hex, bảng màu có sẵn tông da và bảng của riêng bạn, cùng quả cầu tone để chọn sáng tối. Có cả công cụ hút màu.",
    "feat.tools.t": "Công cụ đáng dùng",
    "feat.tools.d": "Đổ màu, gradient, chữ, chèn ảnh, thước thẳng và elip, làm mượt nét, hình khối và nét path, đối xứng dọc / ngang / xoay tròn, lasso, đũa thần, biến đổi, bóp méo, chỉnh màu, curves, cắt và đổi kích thước canvas.",
    "feat.layers.t": "Layer thật sự",
    "feat.layers.d": "Layer raster và vector với blend mode, độ mờ, khoá vùng trong suốt, clipping và mask. Nét vẽ trên layer vector giữ nguyên dạng path có thể chỉnh sửa tới tận Illustrator.",
    "feat.private.t": "Offline và riêng tư",
    "feat.private.d": "Ứng dụng thậm chí không xin quyền truy cập Internet. Tranh của bạn nằm trên máy cho tới khi bạn tự xuất ra, có tự động lưu và khôi phục khi app bị tắt đột ngột.",
    "feat.open.t": "Mã nguồn mở",
    "feat.open.d": "Engine Rust viết độc lập cùng giao diện Kotlin / Jetpack Compose, giấy phép MIT hoặc Apache-2.0. Cứ đọc, tự build, tự fork.",
    "shots.kicker": "02 — Xem qua một vòng",
    "shots.title": "Ảnh chụp thật, không phải mockup.",
    "shots.color": "Vòng màu + bảng màu",
    "shots.tools": "Công cụ, tab Create",
    "shots.export": "Xuất file: Display P3, 16-bit",
    "shots.color.alt": "Bảng màu với vòng hue, ô vuông độ bão hoà và độ sáng, cùng một hàng mẫu màu.",
    "shots.tools.alt": "Bảng công cụ, tab Create: đổ màu, gradient, chữ, ảnh, hút màu, thước, hình khối và đối xứng.",
    "shots.export.alt": "Hộp xuất cho Photoshop đang chọn Display P3 và 16-bit; phần tóm tắt ghi rõ pixel được chuyển từ sRGB theo relative colorimetric và profile được nhúng.",
    "color.kicker": "03 — Màu sắc nói rõ được",
    "color.title": "File ghi rõ chính xác nó là màu gì.",
    "color.lede": "Mọi file xuất ra đều mang theo profile ICC. Bạn chọn không gian màu cho file, và trước khi xuất Veskra cho biết phần nào sẽ được chuyển đổi, phần nào có thể bị cắt màu.",
    "color.l1": "PSD giữ layer, 8 hoặc 16-bit",
    "color.l2": "SVG / PDF cho Illustrator: nét vector chỉnh sửa được, giữ nguyên diện mạo gốc, hoặc cả hai",
    "color.l3": "Xuất ra sRGB, Display P3 hoặc Adobe RGB (1998) compatible, chuyển đổi kiểu relative colorimetric",
    "color.l4": "Tóm tắt trước và sau mỗi lần xuất: số path vector, số layer bị raster hoá, số pixel bị cắt màu",
    "color.note": "Nói thật: màn hình tablet không phải màn hình đã hiệu chỉnh. Canvas dùng sRGB, và màu bạn thấy phụ thuộc vào màn hình — thứ giữ chính xác là file được xuất ra.",
    "color.s1": "Giữ như canvas",
    "dl.kicker": "04 — Tải về",
    "dl.title": "Tải Veskra",
    "dl.latest": "Bản mới nhất:",
    "dl.rec": "Khuyên dùng",
    "dl.arm": "Tablet và điện thoại Android — gần như mọi máy đang bán hiện nay.",
    "dl.uni": "Chạy được trên cả hai; chọn bản này nếu không chắc.",
    "dl.x86": "Dành cho Android emulator trên máy tính.",
    "dl.step1": "Tải file APK về tablet.",
    "dl.step2": "Mở file và cho phép “Cài ứng dụng không rõ nguồn gốc” cho trình duyệt hoặc trình quản lý file khi Android hỏi.",
    "dl.step3": "Mở Veskra và tạo canvas mới.",
    "dl.checks": "Kiểm tra file tải về:",
    "dl.all": "Tất cả các bản",
    "dl.warn.t": "Đây là bản alpha.",
    "dl.warn.d": "Hiện mới chỉ được thử trên emulator tablet Android, chưa thử trên bút cảm ứng thật, và file xuất ra chưa được kiểm tra trong Photoshop hay Illustrator. Sẽ còn chỗ chưa mượt — hãy báo lỗi trên GitHub giúp nhé.",
    "foot.line": "Veskra — ứng dụng vẽ độc lập, mã nguồn mở cho tablet Android.",
    "foot.issues": "Báo lỗi",
    "foot.license": "MIT / Apache-2.0",
    "foot.fine": "“Veskra” là tên tạm thời. Photoshop và Illustrator là thương hiệu của Adobe; Veskra không liên kết với Adobe.",
  };

  const META = {
    en: {
      title: "Veskra — open-source drawing app for Android tablets",
      description: "Veskra is a free, open-source, offline drawing app for Android tablets, with layered PSD and Illustrator-ready SVG/PDF export that keeps track of color.",
      date: (d) => d.toLocaleDateString("en", { year: "numeric", month: "short", day: "numeric" }),
    },
    vi: {
      title: "Veskra — ứng dụng vẽ mã nguồn mở cho tablet Android",
      description: "Veskra là ứng dụng vẽ miễn phí, mã nguồn mở, chạy offline cho tablet Android, xuất PSD nhiều layer và SVG/PDF cho Illustrator kèm thông tin màu chính xác.",
      date: (d) => d.toLocaleDateString("vi", { year: "numeric", month: "numeric", day: "numeric" }),
    },
  };

  // The English text is the markup itself; remember it so switching back restores it.
  const english = new Map();
  document.querySelectorAll("[data-i18n]").forEach((el) => english.set(el, el.textContent));
  const englishAlt = new Map();
  document.querySelectorAll("[data-i18n-alt]").forEach((el) => englishAlt.set(el, el.getAttribute("alt")));

  let lang = "en";
  let releaseDate = null;

  function storedLang() {
    try {
      return localStorage.getItem("veskra-lang");
    } catch {
      return null;
    }
  }

  function initialLang() {
    const q = new URLSearchParams(location.search).get("lang");
    if (q === "en" || q === "vi") return q;
    const saved = storedLang();
    if (saved === "en" || saved === "vi") return saved;
    return (navigator.languages || [navigator.language || ""]).some((l) => /^vi\b/i.test(l)) ? "vi" : "en";
  }

  function apply(next) {
    lang = next;
    document.documentElement.lang = lang;
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      el.textContent = lang === "vi" && VI[key] ? VI[key] : english.get(el);
    });
    document.querySelectorAll("[data-i18n-alt]").forEach((el) => {
      const key = el.getAttribute("data-i18n-alt");
      el.setAttribute("alt", lang === "vi" && VI[key] ? VI[key] : englishAlt.get(el));
    });
    document.querySelectorAll(".lang button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.lang === lang)));
    document.title = META[lang].title;
    document.querySelector('meta[name="description"]').setAttribute("content", META[lang].description);
    showDate();
  }

  function showDate() {
    const el = document.querySelector('[data-release="date"]');
    if (el && releaseDate) el.textContent = "· " + META[lang].date(releaseDate);
  }

  document.querySelectorAll(".lang button").forEach((b) =>
    b.addEventListener("click", () => {
      apply(b.dataset.lang);
      try {
        localStorage.setItem("veskra-lang", lang);
      } catch {
        /* private mode: the choice just isn't remembered */
      }
    }),
  );

  apply(initialLang());

  // Newest release, pre-releases included (GitHub's /releases/latest skips them).
  async function loadRelease() {
    const res = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=10`, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!res.ok) return;
    const release = (await res.json()).find((r) => !r.draft && r.assets && r.assets.some((a) => a.name.endsWith(".apk")));
    if (!release) return;

    document.querySelectorAll('[data-release="version"]').forEach((el) => (el.textContent = release.tag_name));
    document.querySelectorAll('[data-release="link"]').forEach((el) => (el.href = release.html_url));
    releaseDate = new Date(release.published_at || release.created_at);
    showDate();

    for (const [key, suffix] of Object.entries(ASSET_SUFFIX)) {
      const asset = release.assets.find((a) => a.name.endsWith(suffix));
      if (!asset) continue;
      document.querySelectorAll(`[data-release-asset="${key}"]`).forEach((el) => (el.href = asset.browser_download_url));
      const mb = (asset.size / (1024 * 1024)).toFixed(1) + " MB";
      document.querySelectorAll(`[data-release-size="${key}"]`).forEach((el) => (el.textContent = mb));
    }
  }

  loadRelease().catch(() => {
    /* offline or rate-limited: the links already point at a published build */
  });
})();
