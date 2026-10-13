// Veskra landing page: language switch (EN / VI) and the version / size labels of the bundled
// release. Without JavaScript the page is English and the download links still work.
(() => {
  "use strict";

  const VI = {
    "skip": "Bỏ qua, tới nội dung",
    "nav.features": "Tính năng",
    "nav.tour": "Ảnh chụp",
    "nav.color": "Màu sắc",
    "nav.download": "Tải về",
    "nav.get": "Tải về",
    "hero.badge": "Bản thử nghiệm alpha · Android 10+",
    "hero.title1": "Vẽ trên tablet.",
    "hero.title2": "Xuất file chuẩn studio.",
    "hero.lede": "Ứng dụng vẽ miễn phí, mã nguồn mở cho tablet Android. Chạy offline, nhường gần trọn màn hình cho bức vẽ, và chuyển tác phẩm sang Photoshop, Illustrator dưới dạng PSD nhiều layer và SVG/PDF — kèm sẵn profile màu trong file.",
    "hero.cta": "Tải APK",
    "hero.other": "Bản khác",
    "hero.fine": "Dành cho đa số tablet và điện thoại (arm64). Miễn phí · Không cần tài khoản · Không quảng cáo · Tranh không bao giờ rời khỏi máy.",
    "hero.alt": "Màn hình vẽ của Veskra trên tablet Android: đồng cỏ vẽ bằng brush cỏ, hoa và airbrush dưới bầu trời xanh và mặt trời vàng, các nút nổi ở góc và thanh dock phía dưới.",
    "hero.sticker": "viết bằng Rust",
    "feat.kicker": "01 — Có gì bên trong",
    "feat.title": "Đủ công cụ cần dùng, không có thứ phải lướt qua.",
    "feat.canvas.t": "Màn hình dành cho bức vẽ",
    "feat.canvas.d": "Các nút nằm gọn ở góc, thanh dock nhỏ giữ brush, smudge, tẩy và hai màu. Mọi thứ khác chỉ mở ra khi cần và tự đóng khi bạn quay lại vẽ.",
    "feat.brushes.t": "70 brush, và brush của bạn",
    "feat.brushes.d": "Chì, mực, thư pháp, marker, sơn, màu nước, airbrush, phấn và pastel, thiên nhiên (cỏ, lá, lông, tóc), hiệu ứng (lấp lánh, bokeh, sương, mưa, tuyết) và hoa văn. Tự tạo brush trong trình chỉnh brush — đầu cọ dạng ảnh, vân giấy, đường cong lực nhấn, độ nghiêng và tốc độ, jitter — hoặc nhập brush của Infinite Painter (.przp, .prbr).",
    "feat.print.t": "Sẵn sàng để in",
    "feat.print.d": "Xuất PSD và PDF hệ CMYK qua profile in offset FOGRA39 hoặc file .icc của chính nhà in, kèm tổng lượng mực và những màu không in chính xác được, hiện ra trước khi xuất.",
    "feat.handoff.t": "Chuyển sang Photoshop & Illustrator",
    "feat.handoff.d": "PSD giữ layer 8 hoặc 16-bit, SVG / PDF cho Illustrator với nét vector chỉnh sửa được, giữ nguyên diện mạo gốc, hoặc cả hai — file nào cũng mang theo profile màu.",
    "feat.picker.t": "Bốn cách chọn màu",
    "feat.picker.d": "Vòng màu, thanh trượt HSB / RGB / LAB / CMYK kèm mã hex, bảng màu có sẵn tông da và bảng của riêng bạn, cùng quả cầu tone để chọn sáng tối. Có cả công cụ hút màu.",
    "feat.tools.t": "Công cụ đáng dùng",
    "feat.tools.d": "Đổ màu, gradient, chữ, chèn ảnh, thước thẳng và elip, làm mượt nét, hình khối và nét path, đối xứng dọc / ngang / xoay tròn, lasso, đũa thần, biến đổi, bóp méo, chỉnh màu, curves, cắt và đổi kích thước canvas.",
    "feat.layers.t": "Layer thật sự",
    "feat.layers.d": "Layer raster và vector với blend mode, độ mờ, khoá vùng trong suốt, clipping và mask. Nét vẽ trên layer vector giữ nguyên dạng path có thể chỉnh sửa tới tận Illustrator.",
    "feat.private.t": "Offline và riêng tư",
    "feat.private.d": "Mọi thứ chạy offline. App chỉ lên mạng khi bạn bấm “Check for updates” để tải bản mới từ trang này. Tranh của bạn nằm trên máy cho tới khi bạn tự xuất ra, có tự động lưu và khôi phục khi app bị tắt đột ngột.",
    "feat.open.t": "Mã nguồn mở",
    "feat.open.d": "Engine Rust viết độc lập cùng giao diện Kotlin / Jetpack Compose, giấy phép MIT hoặc Apache-2.0. Cứ đọc, tự build, tự fork.",
    "shots.kicker": "02 — Xem qua một vòng",
    "shots.title": "Đủ bộ công cụ, toàn ảnh chụp thật.",
    "shots.lede": "Vẽ và chụp ngay trong app trên emulator tablet Android, không có mockup nào. Bấm vào ảnh để xem cỡ lớn.",
    "shots.filter": "Lọc ảnh chụp",
    "shots.tab.all": "Tất cả",
    "shots.tab.brushes": "Brush",
    "shots.tab.draw": "Vẽ",
    "shots.tab.edit": "Chỉnh sửa",
    "shots.tab.layers": "Layer",
    "shots.tab.color": "Màu",
    "shots.tab.export": "Xuất file",
    "shots.prev": "Ảnh trước",
    "shots.next": "Ảnh sau",
    "shots.close": "Đóng",
    "shots.brushes.t": "Chọn brush",
    "shots.brushes.d": "70 brush trong 11 nhóm. Mỗi thẻ có hình đầu cọ và nét mẫu theo màu đang chọn.",
    "shots.brushes.alt": "Menu chọn brush ở tab Thiên nhiên: cỏ, tán lá, lá rơi, hoa, cành, lông thú, mỗi brush có hình đầu cọ và nét mẫu màu xanh lá.",
    "shots.brushes_wc.t": "Brush màu nước",
    "shots.brushes_wc.d": "Cọ tròn, loang nước, hạt lắng, cọ khô, viền loang và vân muối.",
    "shots.brushes_wc.alt": "Menu chọn brush ở tab Màu nước với sáu brush màu nước tô màu xanh dương, bên dưới là thanh chỉnh size.",
    "shots.brush_editor.t": "Trình chỉnh brush",
    "shots.brush_editor.d": "Đầu cọ, nét, đường cong lực nhấn / độ nghiêng / tốc độ, vân giấy và jitter, có nét xem trước trực tiếp.",
    "shots.brush_editor.alt": "Trình chỉnh brush ở tab Dynamics: nét xem trước, các đầu vào lực nhấn, độ nghiêng, tốc độ và khoảng size kèm đường cong phản hồi.",
    "shots.tools_create.t": "Công cụ tạo",
    "shots.tools_create.d": "Đổ màu, gradient, chữ, chèn ảnh, hút màu, thước thẳng / elip / lazy, hình khối, path và đối xứng.",
    "shots.tools_create.alt": "Bảng công cụ, tab Create, trên bức tranh đồng cỏ: đổ màu, gradient, chữ, ảnh, hút màu, thước, hình khối và đối xứng.",
    "shots.symmetry.t": "Đối xứng xoay tròn",
    "shots.symmetry.d": "Vẽ một lát, có cả bông mandala. Có cả đối xứng dọc và ngang.",
    "shots.symmetry.alt": "Một mandala màu tím, đỏ và vàng vẽ bằng đối xứng xoay tròn; các đường dẫn xanh chia canvas vuông thành sáu lát.",
    "shots.tools_edit.t": "Công cụ chỉnh sửa",
    "shots.tools_edit.d": "Chọn vùng lasso, chữ nhật, tròn và đũa thần, biến đổi, bóp méo, chỉnh màu, curves, cắt và đổi kích thước.",
    "shots.tools_edit.alt": "Bảng công cụ, tab Edit: chọn vùng lasso, chữ nhật, tròn và đũa thần, biến đổi và bóp méo, chỉnh màu và curves, cắt và đổi kích thước.",
    "shots.transform.t": "Biến đổi vùng chọn",
    "shots.transform.d": "Chọn mặt trời, kéo để di chuyển, kéo góc để phóng to thu nhỏ, kéo nút trên cùng để xoay.",
    "shots.transform.alt": "Mặt trời được chọn bằng vùng chọn tròn và đang được di chuyển, phóng to bằng các nút biến đổi.",
    "shots.curves.t": "Curves",
    "shots.curves.d": "Curves RGB hoặc từng kênh, xem trước trực tiếp trên layer.",
    "shots.curves.alt": "Bảng curves với đường cong RGB hình chữ S; những ngọn đồi phía sau đã đậm tương phản hơn.",
    "shots.layers.t": "Layer",
    "shots.layers.d": "Blend mode, độ mờ, khoá vùng trong suốt, clipping, mask từ vùng chọn; layer raster và vector.",
    "shots.layers.alt": "Bảng layer: Sky details, Meadow, Hills và Sky, kèm độ mờ, blend mode, khoá vùng trong suốt và clip vào layer dưới.",
    "shots.color_wheel.t": "Vòng màu",
    "shots.color_wheel.d": "Vòng hue, ô vuông độ bão hoà / độ sáng, bảng màu ngay bên dưới.",
    "shots.color_wheel.alt": "Bảng màu với vòng hue, ô vuông độ bão hoà và độ sáng, cùng bảng màu Basics.",
    "shots.color_cmyk.t": "Thanh trượt CMYK",
    "shots.color_cmyk.d": "Giá trị mực qua profile in, kèm cảnh báo khi màu không in chính xác được.",
    "shots.color_cmyk.alt": "Thanh trượt CMYK cho một màu cam rực, kèm cảnh báo màu này không in chính xác được với profile FOGRA39, và tổng lượng mực.",
    "shots.color_sphere.t": "Quả cầu tone",
    "shots.color_sphere.d": "Chọn sắc tối, tone và sắc sáng của một màu trên quả cầu được chiếu sáng.",
    "shots.color_sphere.alt": "Quả cầu tone: quả cầu cam được chiếu sáng từ góc trên bên trái, kèm các thanh hue, sắc tối và sắc sáng.",
    "shots.color_swatches.t": "Mẫu màu",
    "shots.color_swatches.d": "Màu vừa dùng, bảng màu có sẵn kể cả tông da, và bảng của riêng bạn.",
    "shots.color_swatches.alt": "Tab mẫu màu với các màu vừa dùng trong tranh đồng cỏ và bảng màu Basics.",
    "shots.export_p3.t": "Xuất cho Photoshop",
    "shots.export_p3.d": "Display P3, 16-bit, nhúng profile. Phần tóm tắt cho biết cái gì được chuyển đổi.",
    "shots.export_p3.alt": "Hộp xuất cho Photoshop đang chọn Display P3 và 16-bit; phần tóm tắt ghi rõ pixel được chuyển từ sRGB theo relative colorimetric và profile được nhúng.",
    "shots.export_cmyk.t": "Xuất để in",
    "shots.export_cmyk.d": "PSD / PDF hệ CMYK qua FOGRA39 hoặc file .icc của nhà in, chọn intent và bù điểm đen.",
    "shots.export_cmyk.alt": "Xuất cho Photoshop hệ CMYK: profile in FOGRA39 có sẵn, nút dùng profile riêng của nhà in, cắt hoặc nén màu ngoài gamut, và bù điểm đen.",
    "color.kicker": "03 — Màu sắc nói rõ được",
    "color.title": "File ghi rõ chính xác nó là màu gì.",
    "color.lede": "Mọi file xuất ra đều mang theo profile ICC. Bạn chọn không gian màu cho file, và trước khi xuất Veskra cho biết phần nào sẽ được chuyển đổi, phần nào có thể bị cắt màu.",
    "color.l1": "PSD giữ layer, 8 hoặc 16-bit",
    "color.l2": "SVG / PDF cho Illustrator: nét vector chỉnh sửa được, giữ nguyên diện mạo gốc, hoặc cả hai",
    "color.l3": "Xuất ra sRGB, Display P3 hoặc Adobe RGB (1998) compatible, chuyển đổi kiểu relative colorimetric",
    "color.l5": "CMYK để in trong PSD và PDF: có sẵn profile in offset FOGRA39, hoặc dùng file .icc của nhà in; chọn relative colorimetric hoặc perceptual, có bù điểm đen",
    "color.s5": "CMYK để in",
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
    "dl.update": "Đã có bản 0.1.0-alpha.5 trở lên? Bấm ⋯ → Check for updates (hoặc nút ở cuối màn Gallery): Veskra tải bản mới, kiểm tra rồi nhờ Android cài đè lên bản cũ, giữ nguyên tranh của bạn.",
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
  const englishLabel = new Map();
  document.querySelectorAll("[data-i18n-label]").forEach((el) => englishLabel.set(el, el.getAttribute("aria-label")));

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
    document.querySelectorAll("[data-i18n-label]").forEach((el) => {
      const key = el.getAttribute("data-i18n-label");
      el.setAttribute("aria-label", lang === "vi" && VI[key] ? VI[key] : englishLabel.get(el));
    });
    document.querySelectorAll(".lang button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.lang === lang)));
    document.title = META[lang].title;
    document.querySelector('meta[name="description"]').setAttribute("content", META[lang].description);
    showDate();
    if (lightbox?.open) showShot(current);
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

  // Tour: filter chips and a lightbox. Without JavaScript every screenshot shows and opens as a file.
  const shots = [...document.querySelectorAll(".shot")];
  const lightbox = document.querySelector(".lightbox");
  let current = 0;

  document.querySelectorAll(".tour-tabs button").forEach((b) =>
    b.addEventListener("click", () => {
      const f = b.dataset.filter;
      document.querySelectorAll(".tour-tabs button").forEach((o) => o.setAttribute("aria-pressed", String(o === b)));
      shots.forEach((s) => (s.hidden = f !== "all" && s.dataset.group !== f));
      const strip = document.querySelector(".shots");
      if (strip) strip.scrollLeft = 0;
    }),
  );

  const visible = () => shots.filter((s) => !s.hidden);

  function showShot(i) {
    const list = visible();
    if (!list.length) return;
    current = (i + list.length) % list.length;
    const fig = list[current];
    const img = lightbox.querySelector("img");
    img.src = fig.querySelector("a").getAttribute("href");
    img.alt = fig.querySelector("img").getAttribute("alt");
    const title = lightbox.querySelector("p");
    title.replaceChildren();
    const b = document.createElement("b");
    b.textContent = fig.querySelector("figcaption b").textContent;
    title.append(b, fig.querySelector("figcaption span").textContent);
  }

  if (lightbox && typeof lightbox.showModal === "function") {
    shots.forEach((fig) =>
      fig.querySelector("a").addEventListener("click", (e) => {
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
        e.preventDefault();
        showShot(visible().indexOf(fig));
        lightbox.showModal();
      }),
    );
    lightbox.querySelector(".lb-prev").addEventListener("click", () => showShot(current - 1));
    lightbox.querySelector(".lb-next").addEventListener("click", () => showShot(current + 1));
    lightbox.querySelector(".lb-close").addEventListener("click", () => lightbox.close());
    // a click on the dimmed backdrop lands on the dialog element itself
    lightbox.addEventListener("click", (e) => {
      if (e.target === lightbox) lightbox.close();
    });
    lightbox.addEventListener("keydown", (e) => {
      if (e.key === "ArrowLeft") showShot(current - 1);
      if (e.key === "ArrowRight") showShot(current + 1);
    });
    let touchX = null;
    lightbox.addEventListener("touchstart", (e) => (touchX = e.touches[0].clientX), { passive: true });
    lightbox.addEventListener("touchend", (e) => {
      if (touchX === null) return;
      const dx = e.changedTouches[0].clientX - touchX;
      touchX = null;
      if (Math.abs(dx) > 50) showShot(current + (dx < 0 ? 1 : -1));
    });
  }

  apply(initialLang());

  // download/latest.json is written at deploy time from the newest GitHub release (pre-releases
  // included) and the APKs sit next to it on this domain, so the buttons download directly.
  async function loadRelease() {
    const res = await fetch("download/latest.json", { cache: "no-cache" });
    if (!res.ok) return;
    const release = await res.json();

    document.querySelectorAll('[data-release="version"]').forEach((el) => (el.textContent = release.tag));
    document.querySelectorAll('[data-release="link"]').forEach((el) => (el.href = release.url));
    releaseDate = new Date(release.published);
    showDate();

    for (const [key, asset] of Object.entries(release.assets)) {
      document.querySelectorAll(`[data-release-asset="${key}"]`).forEach((el) => {
        el.href = "download/" + asset.file;
        el.setAttribute("download", asset.name);
      });
      const mb = (asset.size / (1024 * 1024)).toFixed(1) + " MB";
      document.querySelectorAll(`[data-release-size="${key}"]`).forEach((el) => (el.textContent = mb));
    }
  }

  loadRelease().catch(() => {
    /* the links already point at the files bundled with this deploy */
  });
})();
