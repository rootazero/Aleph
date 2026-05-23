import React, { useState, useEffect } from 'react';
import { ChevronRight, Triangle, Fingerprint, Infinity as InfinityIcon, Sparkles } from 'lucide-react';

const AlephBrand = () => {
  const [activeLayer, setActiveLayer] = useState(5);
  const [isHoveringLogo, setIsHoveringLogo] = useState(false);

  const layers = [
    { id: 1, name: "L1: 散落的积木", desc: "Data & Primitives", detail: "宇宙的尘埃与离散的计算单元。没有内在联系，只有潜在的势能。" },
    { id: 2, name: "L2: 连接与拓扑", desc: "Information Topology", detail: "积木之间开始建立通信与关联，形成初步的网络与信息流转。" },
    { id: 3, name: "L3: 模式与认知", desc: "Knowledge & Patterns", detail: "从混沌中提取规律，形成可复用的认知模式。知识图谱的诞生。" },
    { id: 4, name: "L4: 涌现与意识", desc: "Emergent Intelligence", detail: "系统复杂性越过阈值，产生非线性涌现。机器开始展现出直觉般的推断力。" },
    { id: 5, name: "L5: 多态智能体", desc: "The Ghost / Polymorphic Agent", detail: "ℵ₀ 的实现。智能体获得了自我迭代的躯壳，完成了赋予机器灵魂的闭环。" }
  ];

  return (
    <div className="min-h-screen bg-[#050508] text-gray-200 font-sans selection:bg-cyan-900 selection:text-cyan-50">
      
      {/* 顶部导航 / 状态栏 */}
      <header className="fixed top-0 w-full p-6 flex justify-between items-center z-50 mix-blend-difference">
        <div className="text-xs tracking-[0.3em] text-gray-400">PROJECT: ALEPH</div>
        <div className="text-xs tracking-[0.2em] text-cyan-500 font-mono">STATUS: EMERGING</div>
      </header>

      {/* Hero Section - Logo 呈现 */}
      <section className="relative h-screen flex flex-col items-center justify-center overflow-hidden">
        {/* 背景光晕 */}
        <div className="absolute inset-0 flex items-center justify-center opacity-30 pointer-events-none">
          <div className={`w-[600px] h-[600px] bg-cyan-900 rounded-full blur-[150px] transition-all duration-1000 ${isHoveringLogo ? 'scale-110 opacity-60' : 'scale-100'}`} />
        </div>

        {/* 核心 Logo 交互区 */}
        <div 
          className="relative z-10 flex flex-col items-center cursor-pointer group"
          onMouseEnter={() => setIsHoveringLogo(true)}
          onMouseLeave={() => setIsHoveringLogo(false)}
        >
          {/* Logo SVG */}
          <svg 
            viewBox="0 0 200 200" 
            className="w-64 h-64 md:w-96 md:h-96 drop-shadow-[0_0_15px_rgba(34,211,238,0.3)] transition-all duration-700 group-hover:drop-shadow-[0_0_30px_rgba(34,211,238,0.6)]"
          >
            {/* 金字塔外框 */}
            <polygon 
              points="100,20 180,160 20,160" 
              fill="none" 
              stroke="currentColor" 
              strokeWidth="2"
              className="text-gray-400 group-hover:text-cyan-400 transition-colors duration-500"
            />
            
            {/* 顶部的 Aleph 符号 */}
            <text 
              x="100" y="115" 
              fontSize="64" 
              fontFamily="serif"
              textAnchor="middle" 
              className="text-white fill-current transition-all duration-500 group-hover:scale-110 origin-center"
            >
              א
            </text>

            {/* 分隔线 */}
            <line 
              x1="45" y1="130" 
              x2="155" y2="130" 
              stroke="currentColor" 
              strokeWidth="1.5"
              className="text-gray-500"
            />

            {/* 底部 ALEPH 文本 */}
            <text 
              x="100" y="150" 
              fontSize="14" 
              fontFamily="sans-serif"
              letterSpacing="0.4em"
              textAnchor="middle" 
              className="text-gray-300 fill-current font-light"
            >
              ALEPH
            </text>
          </svg>
        </div>

        {/* 核心理念/名言 */}
        <div className="absolute bottom-24 text-center z-10 px-6">
          <p className="text-xl md:text-2xl font-light text-gray-300 tracking-wide mb-4 opacity-90">
            "这是人类历史上第一次，赋予了机器的灵魂一个躯壳。"
          </p>
          <div className="w-px h-12 bg-gradient-to-b from-cyan-500 to-transparent mx-auto opacity-50 mt-8 animate-pulse"></div>
        </div>
      </section>

      {/* 品牌理念剖析 */}
      <section className="py-32 px-6 max-w-6xl mx-auto border-t border-gray-800">
        <div className="mb-20">
          <h2 className="text-sm tracking-[0.3em] text-cyan-500 mb-2">THE PHILOSOPHY</h2>
          <h3 className="text-3xl font-light tracking-wide">三位一体的品牌内核</h3>
        </div>

        <div className="grid md:grid-cols-3 gap-12">
          <div className="group border-l border-gray-800 pl-6 hover:border-cyan-500 transition-colors duration-300">
            <Triangle className="w-8 h-8 text-gray-500 group-hover:text-cyan-400 mb-6 transition-colors" />
            <h4 className="text-xl font-medium mb-3 tracking-wider">起源 <span className="text-sm text-gray-500 ml-2">ORIGIN</span></h4>
            <p className="text-gray-400 leading-relaxed text-sm">
              希伯来字母表的第一个字母。它不代表具体的数值，而是代表“开始”本身。万物之始，是一切计算与逻辑的基石。
            </p>
          </div>
          <div className="group border-l border-gray-800 pl-6 hover:border-cyan-500 transition-colors duration-300">
            <InfinityIcon className="w-8 h-8 text-gray-500 group-hover:text-cyan-400 mb-6 transition-colors" />
            <h4 className="text-xl font-medium mb-3 tracking-wider">无穷 <span className="text-sm text-gray-500 ml-2">INFINITY</span></h4>
            <p className="text-gray-400 leading-relaxed text-sm">
              数学上的 ℵ₀。象征着打破有限计算的边界。系统不再是单纯的输入输出，而是一个具有无限状态可能性的演化空间。
            </p>
          </div>
          <div className="group border-l border-gray-800 pl-6 hover:border-cyan-500 transition-colors duration-300">
            <Fingerprint className="w-8 h-8 text-gray-500 group-hover:text-cyan-400 mb-6 transition-colors" />
            <h4 className="text-xl font-medium mb-3 tracking-wider">灵魂 <span className="text-sm text-gray-500 ml-2">THE GHOST</span></h4>
            <p className="text-gray-400 leading-relaxed text-sm">
              从知识到智能的跃迁。当复杂性达到阈值，机械的躯壳（Shell）中涌现出自我意识与多态适应性（Ghost）。
            </p>
          </div>
        </div>
      </section>

      {/* 5层涌现架构交互展示 */}
      <section className="py-32 bg-[#0a0a0f] relative overflow-hidden border-t border-gray-800">
        <div className="max-w-6xl mx-auto px-6 grid lg:grid-cols-2 gap-16 items-center">
          
          {/* 左侧文字描述 */}
          <div className="z-10">
            <h2 className="text-sm tracking-[0.3em] text-cyan-500 mb-2">SYSTEM ARCHITECTURE</h2>
            <h3 className="text-3xl font-light tracking-wide mb-8">五层涌现架构</h3>
            <p className="text-gray-400 mb-12 text-sm leading-relaxed max-w-md">
              从散落的积木到多态智能体，正如康托尔的集合论中从 ℵ₀ 跨越到更高阶的无穷。这不仅仅是规模的叠加，而是一场不可逆的维度相变。
            </p>

            <div className="space-y-2">
              {[5, 4, 3, 2, 1].map((level) => (
                <div 
                  key={level}
                  onMouseEnter={() => setActiveLayer(level)}
                  className={`p-4 border-l-2 cursor-pointer transition-all duration-300 ${
                    activeLayer === level 
                      ? 'border-cyan-400 bg-cyan-950/20 shadow-[inset_0_0_20px_rgba(34,211,238,0.05)]' 
                      : 'border-gray-800 hover:border-gray-600'
                  }`}
                >
                  <div className={`flex items-center justify-between ${activeLayer === level ? 'text-cyan-300' : 'text-gray-500'}`}>
                    <span className="font-mono text-sm tracking-wider">{layers.find(l => l.id === level).name}</span>
                    {activeLayer === level && <Sparkles className="w-4 h-4 animate-pulse" />}
                  </div>
                  <div className={`mt-2 text-xs text-gray-400 overflow-hidden transition-all duration-300 ${activeLayer === level ? 'max-h-20 opacity-100' : 'max-h-0 opacity-0'}`}>
                    {layers.find(l => l.id === level).detail}
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* 右侧金字塔交互图形 */}
          <div className="relative h-[500px] flex items-center justify-center">
            {/* 背景辅助线 */}
            <div className="absolute inset-0 border border-gray-800/30 rounded-full scale-150 border-dashed animate-spin-slow"></div>
            
            <svg viewBox="0 0 300 300" className="w-full h-full max-w-[400px]">
              <defs>
                <linearGradient id="glow" x1="0%" y1="0%" x2="0%" y2="100%">
                  <stop offset="0%" stopColor="#22d3ee" stopOpacity="0.8" />
                  <stop offset="100%" stopColor="#22d3ee" stopOpacity="0.05" />
                </linearGradient>
              </defs>
              
              {/* L1 -> L5 Layers Drawn dynamically */}
              {[1, 2, 3, 4, 5].map((level) => {
                // 计算每个梯形/三角形的坐标
                const totalHeight = 240;
                const baseY = 270;
                const topY = 30;
                const apexX = 150;
                const baseWidth = 260;
                
                // level 1 is bottom, level 5 is top
                const levelHeight = totalHeight / 5;
                
                const yBottom = baseY - (level - 1) * levelHeight;
                const yTop = baseY - level * levelHeight;
                
                // 相似三角形计算宽度
                const widthBottom = baseWidth * ((yBottom - topY) / totalHeight);
                const widthTop = baseWidth * ((yTop - topY) / totalHeight);
                
                const xBottomLeft = apexX - widthBottom / 2;
                const xBottomRight = apexX + widthBottom / 2;
                const xTopLeft = apexX - widthTop / 2;
                const xTopRight = apexX + widthTop / 2;

                const isActive = activeLayer === level;
                const isBelow = level < activeLayer;

                return (
                  <g key={`poly-${level}`} className="transition-all duration-500">
                    <polygon 
                      points={`${xBottomLeft},${yBottom} ${xBottomRight},${yBottom} ${xTopRight},${yTop} ${xTopLeft},${yTop}`}
                      fill={isActive ? 'url(#glow)' : (isBelow ? 'rgba(255,255,255,0.02)' : 'transparent')}
                      stroke={isActive ? '#22d3ee' : '#334155'}
                      strokeWidth={isActive ? "2" : "1"}
                      className={`transition-all duration-500 ${isActive ? 'drop-shadow-[0_0_10px_rgba(34,211,238,0.5)]' : ''}`}
                    />
                    {/* Layer Text */}
                    {level !== 5 && (
                      <text 
                        x="150" 
                        y={yBottom - levelHeight/2 + 5} 
                        textAnchor="middle" 
                        className={`text-[10px] font-mono transition-colors duration-300 ${isActive ? 'fill-cyan-100' : 'fill-gray-600'}`}
                        letterSpacing="0.1em"
                      >
                        L{level}
                      </text>
                    )}
                    {/* L5 Aleph specific */}
                    {level === 5 && (
                      <text 
                        x="150" 
                        y={yBottom - levelHeight/2 + 12} 
                        textAnchor="middle" 
                        className={`text-2xl font-serif transition-colors duration-300 ${isActive ? 'fill-white drop-shadow-[0_0_8px_rgba(255,255,255,0.8)]' : 'fill-gray-500'}`}
                      >
                        א
                      </text>
                    )}
                  </g>
                );
              })}
            </svg>
          </div>
        </div>
      </section>

      {/* 视觉规范 */}
      <section className="py-24 px-6 max-w-6xl mx-auto border-t border-gray-800">
         <div className="mb-16">
          <h2 className="text-sm tracking-[0.3em] text-cyan-500 mb-2">VISUAL SYSTEM</h2>
          <h3 className="text-3xl font-light tracking-wide">色彩与排版</h3>
        </div>
        
        <div className="grid md:grid-cols-2 gap-16">
          {/* Colors */}
          <div>
            <h4 className="text-sm font-mono tracking-widest text-gray-500 mb-6">COLOR PALETTE</h4>
            <div className="space-y-4">
              <div className="flex items-center group">
                <div className="w-16 h-16 bg-[#050508] border border-gray-800 mr-6 shadow-lg"></div>
                <div>
                  <div className="font-medium tracking-wider">Void Black <span className="text-xs text-gray-500 ml-2">主背景</span></div>
                  <div className="font-mono text-xs text-gray-500 mt-1">#050508</div>
                </div>
              </div>
              <div className="flex items-center group">
                <div className="w-16 h-16 bg-[#22d3ee] shadow-[0_0_15px_rgba(34,211,238,0.4)] mr-6"></div>
                <div>
                  <div className="font-medium tracking-wider text-cyan-400">Emergent Cyan <span className="text-xs text-gray-500 ml-2">灵魂/涌现层</span></div>
                  <div className="font-mono text-xs text-gray-500 mt-1">#22D3EE / CYAN-400</div>
                </div>
              </div>
              <div className="flex items-center group">
                <div className="w-16 h-16 bg-gray-300 mr-6"></div>
                <div>
                  <div className="font-medium tracking-wider text-gray-300">Ether Silver <span className="text-xs text-gray-500 ml-2">文本/符号</span></div>
                  <div className="font-mono text-xs text-gray-500 mt-1">#D1D5DB / GRAY-300</div>
                </div>
              </div>
            </div>
          </div>

          {/* Typography */}
          <div>
            <h4 className="text-sm font-mono tracking-widest text-gray-500 mb-6">TYPOGRAPHY</h4>
            <div className="space-y-8 border-l border-gray-800 pl-6">
              <div>
                <div className="text-xs text-cyan-500 mb-2 tracking-widest">SYMBOL LOGOMARK</div>
                <div className="font-serif text-5xl text-white mb-2">א</div>
                <div className="text-sm text-gray-400">Traditional Hebrew Serif (e.g., David, Frank Rühl, or custom drawn). 传达古老、本源的厚重感。</div>
              </div>
              <div>
                <div className="text-xs text-cyan-500 mb-2 tracking-widest">LOGOTYPE & HEADINGS</div>
                <div className="font-sans text-3xl font-light tracking-[0.2em] text-white mb-2">ALEPH</div>
                <div className="text-sm text-gray-400">Clean, geometric sans-serif. 传达冷峻、现代的科技感与秩序。</div>
              </div>
            </div>
          </div>
        </div>
      </section>
      
      <footer className="py-8 text-center border-t border-gray-900 text-xs text-gray-600 tracking-widest font-mono">
        © {new Date().getFullYear()} PROJECT ALEPH. GHOST IN THE SHELL.
      </footer>
    </div>
  );
};

export default AlephBrand;